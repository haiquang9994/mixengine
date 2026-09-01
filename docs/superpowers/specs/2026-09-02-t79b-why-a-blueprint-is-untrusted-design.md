# T79b — saying why a blueprint is untrusted (design)

Roadmap task **T79b**, phase 8. T78a made `blueprint.import` check a detached minisign signature
against the compiled-in gallery key and write the answer into a column that nothing ever raises.
It writes a *boolean*, and the daemon already knows more than a boolean: a file that arrived with no
signature at all and a file whose signature did not verify are two different events, and only the
second is the one the gallery key exists to catch. Today both produce the same line —

```
untrusted: nothing vouches for it, and nothing will
```

— and the difference survives only in the daemon's own log. This task carries it to the client.

Found by T79a's acceptance run and left alone there rather than widening that task into
`mixengine-proto`.

**Trust stays a decision made once.** Nothing here re-checks a signature, re-reads a file, or moves
a `trusted` column. This adds a *reason* beside an answer that was settled when the row was written.

## Goal

Three sentences where there is one, in the three places a person meets a blueprint's trust:

| | `blueprint import` | `blueprint list` | the `[scaffold]` question |
|---|---|---|---|
| verified | signed by the gallery key | `signed` | This blueprint is signed. |
| nothing came with it | untrusted: nothing came with it to vouch for it, and nothing will | `unsigned` | Nothing vouches for this blueprint. |
| a signature did not verify | untrusted: a signature came with it, and it is not the gallery's | `mismatched` | A signature came with this blueprint and it is not the gallery's. |
| a row older than this task | *(today's sentence, unchanged)* | `untrusted` | Nothing vouches for this blueprint. |

And the reason survives a restart, because it is a column rather than something in a `Result`.

## Scope

**In:** `mixengine-proto` (`SignatureCheck`, one field on `BlueprintSummary` and one on
`BlueprintPlan`); `mixengine-core` (`Trust`, `store::save`'s parameter, `records`, `filed_of`,
`Filed`, `plan`, `gallery`); migration `0015`; `mixengine-daemon` (`vouched_for` answers `Trust`);
`mixengine-cli` (three renderings); `.claude/features/blueprints.md`.

**Out:** any change to what apply *does*. No new flag, no new refusal, no second class of untrusted
blueprint (D8). No re-verification, ever (D10). No `blueprint.delete`, no way to raise a row's trust
— those were settled by T78a and are not reopened here.

## Decisions

### D1 — One value when it is written, two fields when it is read

`mixengine-core::blueprints::trust` gains the enum that decides:

```rust
pub enum Trust { Inherent, Verified, Unsigned, Rejected }

impl Trust {
    pub fn trusted(self) -> bool;                     // Inherent | Verified
    pub fn signature(self) -> Option<SignatureCheck>; // Inherent -> None
}
```

`store::save` takes `trust: Trust` where it took `trusted: bool`, and derives both columns from it.
**The two can never disagree, because they are never set apart.** The alternative — a bool and an
`Option` passed side by side — is two parameters a caller can get out of step, in the one function
that decides whether somebody else's code may run on this machine.

`mixengine-proto` carries the two halves separately:

```rust
pub enum SignatureCheck { Verified, Missing, Rejected }

pub struct BlueprintSummary {
    // ...
    pub trusted: bool,
    #[serde(default, deserialize_with = "lenient", skip_serializing_if = "Option::is_none")]
    pub signature: Option<SignatureCheck>,
}
```

`trusted` stays what it was: **the answer**, the thing behaviour reads. `signature` is the evidence
beside it. Replacing the bool with one five-variant enum was considered and refused: it would break
a field T78a shipped, and its `Builtin`/`Captured` arms would restate `BlueprintSource`, which is
already on every summary.

`vouched_for` in the daemon answers `Trust` instead of `bool`. That is the function that already
knows all three cases and throws two away.

### D2 — `Inherent` is one word for builtin and captured

`BlueprintSource` tells builtin from captured; this enum does not need to. What the two share is
exactly what it records: **no signature check happened, and none was needed** — a capture is this
machine's own, and a blueprint compiled into the binary beside the key that would check it proves
nothing a signature could add (T79's D3). `Trust::Inherent` writes `NULL`, and `None` on the wire.

### D3 — Reading back does not reconcile, and an unknown word is not an error

`records` and `filed_of` read the two columns as they were written and do not derive either from
the other. A hand-edited database holding `trusted = 1` beside `signature = 'rejected'` is answered
with exactly that: **`trusted` governs** — T78a's rule that trust is decided once and never
re-examined — and `signature` is a sentence beside it.

A word the build does not know (a newer daemon's, a hand-edit) reads as `None`, is logged, and does
not fail the call. This is where `signature` parts from `source`: `source` drives behaviour, so
`Error::UnknownBlueprintSource` refuses to guess at it; `signature` drives one sentence, and killing
a whole `blueprint list` over a word that decorates it is the wrong price.

**The same rule on the wire.** `Option<SignatureCheck>` deserializes leniently: an unknown string
becomes `None` rather than a decode failure. Without it, one `mix` older than a future variant would
fail to parse the entire listing — a decoration taking the response down with it. One unknown word,
one reading, at both ends.

### D4 — The overwrite path updates the new column

`save`'s `INSERT … ON CONFLICT (id) DO UPDATE SET` names the columns it refreshes. `signature`
belongs in that list. Left out, re-importing an unsigned file over a verified row would leave
`trusted = 0` beside `signature = 'verified'` — the self-contradicting row D3 declines to reconcile,
manufactured by this build rather than by a hand-edit. A test imports signed, then re-imports
unsigned with `--overwrite`, and reads the reason back as `missing`.

### D5 — Migration `0015`, backfilling only what is knowable

```sql
ALTER TABLE blueprints ADD COLUMN signature TEXT;

UPDATE blueprints SET signature = 'verified' WHERE source = 'imported' AND trusted = 1;
```

An `imported` row with `trusted = 1` can only have come from a signature that verified: `import` is
the only writer of that source, and it sets the column from `vouched_for` alone. An `imported` row
with `trusted = 0` is *one of the two* and the schema does not record which, so it stays `NULL` and
its client says the sentence it says today. **Nothing is guessed.**

The column holds the same word as the wire, on `BlueprintSource::as_str`'s rule: `verified`,
`missing`, `rejected`. A second vocabulary would be a second thing to keep in step.

### D6 — Three sentences, in three places that already exist

The Goal's table, rendered by `mixengine-cli` and nothing else. The third row is the one this task
is for, and it lands in the place it matters most: the question `mix blueprint apply` asks before
running somebody else's command.

**The wording must be true for every failure `trust::verify` folds together.** Its own doc lists
them: "a blueprint edited after it was signed, a signature from another key, or a file that is not a
signature at all". So the sentence is *"a signature came with it, and it is not the gallery's"* —
true of all three. An earlier draft said "these are not the bytes that were signed", which accuses a
colleague who signed with their own key, and a corrupt `.minisig`, of tampering.

The `TRUST` column widens from 10 to 12 to hold `mismatched`.

### D7 — No variant for "signed by another key"

Splitting `Rejected` into "the gallery signed this and the bytes changed" and "somebody else signed
this" would be worth a lot: they are very different stories. The only thing that could tell them
apart is the **key id inside the `.minisig`, and a key id is not authenticated** — whoever edits the
file edits the key id with it. Branching a security message on bytes an attacker writes hands them
the message. Three variants, and a sentence that does not over-claim.

### D8 — The plan carries the reason; nothing about applying changes

`BlueprintPlan` gains the same field, from the same row, so the `[scaffold]` question can say which
of the two it is. That is the whole of the change at apply time.

`--run-untrusted-scaffold` still covers both untrusted kinds. A file whose signature did not verify
does not become a refusal, and does not earn a flag of its own: T78a argued that a failed signature
is not a refusal, and a second gate here would be a new security decision wearing this task's name.
What changes is the sentence, not the gate.

### D9 — `ScaffoldConsent` is not extended (considered, declined)

T78a's consent copies *what was read* — the command, and whether the person was told nobody vouches
for it — so that a blueprint re-imported between the plan and the apply cannot spend a consent given
for a different one. This task makes what was read richer, so the principle argues for pinning
`signature` too. It is declined:

- `command` is already pinned, so the command that runs is the command that was read.
- Both untrusted kinds sit behind one flag, so no privilege is escalated by confusing them.
- The gap is therefore that the *explanation* a person was given could differ from the row's, with
  identical consequences either way.

Written down here so nobody reopens it blind. **If a third trust level is ever added, this is where
it leaks**, and the pinning becomes worth its refusal path.

### D10 — This says nothing about the file on disk

`mix blueprint list` will print `signed` for a row whose `blueprints/<slug>.toml` somebody edited
after it was imported. That is T78a's D16 working as designed and not a hole this task opens: the
row is the truth, and the file beside it is a *rendering* — not the artifact that was signed. A
check made later would be a check against bytes the signer never saw. The column is a record of what
arrived, and it is never a claim about what is on disk now.

## Delivery

One branch, `t79b-why-a-blueprint-is-untrusted`, one commit per task:

1. `mixengine-proto`: `SignatureCheck`, the lenient reader, both fields.
2. `mixengine-core`: `Trust`, migration `0015`, `save`/`records`/`filed_of`/`Filed`, `plan`,
   `gallery`, and `cargo sqlx prepare`.
3. `mixengine-daemon`: `vouched_for` answers `Trust`.
4. `mixengine-cli`: the three renderings.
5. Documentation and the roadmap tick.

## Testing

- **proto** — the three variants round-trip; a summary written before this task decodes with
  `signature: None`; an unknown word decodes as `None` rather than failing.
- **core** — `save(Trust::Rejected)` comes back untrusted with `Rejected` from both `records` and
  `filed_of`; the migration's backfill leaves an `imported`/`trusted = 0` row `NULL`; an unknown
  word in the column reads as `None`.
- **daemon** — import with a good signature, with a signature that does not verify, and with none:
  three reasons. Then `blueprint.list` says the same three, **which is the test that proves the
  column rather than the response** — a test reading only `import`'s answer stays green with the
  migration broken.
- **cli** — the three lines differ, and the fourth (`None`) is today's line unchanged.

## Risks

- **A stale reason beside a fresh answer.** D4's `ON CONFLICT` list is the whole mitigation, and it
  is one line easy to forget; the re-import test exists for it and not for the feature.
- **A newer daemon talking to an older `mix`.** D3's lenient wire reading. Without it this task ages
  into a parse failure the first time a fourth variant is added.
- **Reading the column as a promise about the file.** D10, stated in the feature doc as well as
  here.

## Acceptance

On a machine, in a sandbox home:

```
mix blueprint import shop.toml                 # nothing beside it: untrusted, nothing came with it
                                               # put shop.toml.minisig beside it
mix blueprint import shop.toml --overwrite     # signed by the gallery key
                                               # append one byte to shop.toml
mix blueprint import shop.toml --overwrite     # untrusted: a signature came with it, and it is not
                                               # the gallery's
mix blueprint list                             # signed / unsigned / mismatched, in the TRUST column
mix blueprint apply <slug> --dry-run           # the scaffold question names which one it is
```

and the reason is still there after the daemon is restarted.
