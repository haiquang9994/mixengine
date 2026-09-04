# T86 — the updater key, and what CI does with it (design)

Roadmap task **T86**, phase 9: *"Minisign updater keys: generation, CI signing of artifacts, pubkey
pinned in the app. **No OS code signing** — see [ADR 0005](../../../.claude/decisions/0005-on-demand-elevation.md)
and [updates.md](../../../.claude/features/updates.md)."*

The feature document is [updates.md](../../../.claude/features/updates.md); the release process is
[build-and-release.md](../../../.claude/operations/build-and-release.md), whose *Signing* section has
said since T85 that this half is owed and that a `.sha256` is not a signature and is not offered as
one.

**This task is the trust root of the updater, and nothing else.** T85 produced six artifacts and
nobody's word for them. T88 will consume a signed feed. In between there has to be a key that
exists, a private half only CI holds, a public half compiled into the product, and a pipeline that
cannot publish an artifact it did not sign — which is the whole of what is below.

**Two things this task settles that its own sentence left open**, both argued: the artifacts are
signed **once, on one runner**, and not in each of the five build legs (D4); and a tag does not
publish a release to the world, it assembles a **draft** somebody publishes (D6).

## Goal

A person who downloads `mixengine-0.1.0-linux-x86_64.deb` can run one `minisign -V` against a public
key printed in the documentation and know they have what we built. A daemon that will later fetch an
update has that same key compiled into it. And a release that reaches the outside world cannot have
an unsigned artifact in it, because the job that assembles it counts.

## Scope

**In:**

- The **key**: `updates.key` / `updates.pub`, generated on the machine that cuts releases, private
  half in this repository's Actions secrets, public half committed as `packaging/updates.pub`.
- `mixengine-core::updates`: one module holding `PUBLIC_KEY`, and the test that keeps the three
  copies of that value from drifting apart.
- `packaging/sign.sh`: sign every artifact in a distribution directory, verify each signature
  against the pinned key before returning, and refuse to leave one unsigned.
- `.github/workflows/ci.yml`: a `v*` tag trigger, a `preflight` job, and a `release` job that signs
  what the five build legs made and assembles a **draft** GitHub Release from it.
- A self-test of the signing script in `lint`, run against a throwaway key on every CI run.
- Documentation: `updates.md`, `build-and-release.md`, `packaging/README.md`, the roadmap, and the
  one line in T88's design that this task's ordering makes stale.

**Out:**

- **`latest.json`** — T88's. It lists the update payloads T88's D6 creates, and there is no honest
  way to generate a feed for archives that do not exist. The release job signs *whatever is in the
  distribution directory*, so T88 adds a step that writes the feed and needs no change here.
- **OS code signing** — not purchased ([ADR 0005](../../../.claude/decisions/0005-on-demand-elevation.md)).
  Whether a certificate this project can buy repairs Smart App Control is **T94**, and this task
  changes nothing about that question.
- **The in-elevate verification** — T88a's. It pins its own copy of the key, because
  `workspace_layering.rs` forbids `mixengine-elevate → mixengine-core` (D3).
- **Anything the updater does with a signature** — T88's. This task publishes signatures; it does
  not yet read one at runtime.

## Decisions

### D1 — A third key, and one key for every artifact in a release

`~/.config/mixengine/` already holds two private halves: `minisign.key` signs the package index,
`blueprints.key` signs the blueprint gallery. This adds `updates.key`.

**A third key rather than the index's**, on the gallery key's own argument. The two existing keys are
used from `mixnz/mixengine-packages`, by workflows in that repository, with that repository's Actions
secrets. A compromise there would cost the package index and the gallery; it must not additionally
hand an attacker the right to sign the `mixengined` a machine will run as itself. Different
repository, different workflow, different secret, different blast radius — three reasons and they all
point the same way.

**One key for every artifact in the release, including `mixengine-elevate`.** The helper is the one
binary that runs as root, and the instinct to give it a key of its own is worth stating in order to
refuse it: the helper is built by the same job, from the same commit, in the same repository, and
published in the same release as the daemon. A second key would be the same secret in the same place
under a second name — it would split the *label* and not the blast radius, which is the opposite of
what D1's first paragraph is about. What actually protects the helper is that it is never
auto-updated and that its replacement is verified inside the elevated context (T88a), and neither of
those gets better with a second key.

The layout, one row per key:

| File | Signs | Public half | Secrets, and where |
| --- | --- | --- | --- |
| `minisign.key` | the package index | `minisign.pub`, `core::index::PUBLIC_KEY` | `mixnz/mixengine-packages` |
| `blueprints.key` | the blueprint gallery | `blueprints.pub`, `core::blueprints::trust::PUBLIC_KEY` | `mixnz/mixengine-packages` |
| `updates.key` | **release artifacts** | `packaging/updates.pub`, `core::updates::PUBLIC_KEY` | **`mixnz/mixengine`** |

### D2 — The pin is a literal constant with a test beside it, not `include_str!`

The tempting move is to have one source of truth: `include_str!("../../../packaging/updates.pub")`
and slice the second line out at compile time. It does not survive contact with the file format — a
minisign public key file is an untrusted comment line and then the key, and splitting a `&'static
str` in a `const` context is `const`-fn string surgery to save a test.

So the constant is a literal, the file is committed beside the script that uses it, and a **unit
test in `updates.rs`** ties them together. It asserts three things, and the third is the one worth
having:

1. `packaging/updates.pub`'s second line is exactly `PUBLIC_KEY` — the drift guard.
2. `PUBLIC_KEY` parses as a minisign public key — a broken paste is a test failure and not a
   verification path that can only ever fail.
3. `PUBLIC_KEY` is **not** `index::PUBLIC_KEY` and not `blueprints::trust::PUBLIC_KEY`. A rotation
   done in a hurry pastes the wrong key from the wrong file, and every other check in this design
   passes when it does: the key is valid, the file matches, the signature verifies. Only this one
   notices that the wrong key is being used correctly.

`include_str!` still earns its place inside the test, so a `packaging/updates.pub` that is deleted or
moved is a compile error rather than a test that reads `None` and passes.

### D3 — `core::updates` is created here, holds the constant, and holds nothing else

T88's design (D2) says the key would be generated by T88 *"because a constant that is not a valid
minisign key makes every test in this task untestable"*. The roadmap order settles it the other way
round — T86 comes first and this is its own sentence — so the key arrives here, and T88 finds it
already pinned. That design's D2 is amended to say so.

The module is one constant and its documentation. In particular there is **no `verify()` function**,
and the absence is a decision rather than an omission:

- T88 verifies `latest.json` through `core::index::Client`, which already checks a signature before
  it parses a document, and verifies the payload by the SHA-256 the signed feed carries (T88's D3).
  Neither path wants a second detached verifier.
- T88a verifies `mixengine-elevate` **inside the elevated context**, and
  `crates/mixengine-proto/tests/workspace_layering.rs` does not allow `mixengine-elevate` to depend
  on `mixengine-core`. So that copy of the key, and the four lines of `minisign_verify` around it,
  will live in the helper.

A `verify()` here would therefore have exactly one caller — a test of itself. What this task ships
instead is D2's third assertion written so that adding elevate's copy to it is one line.

### D4 — Signing is one job on one runner

The obvious shape is a step at the end of each `build` leg: the artifacts are already on disk there,
and each leg signs its own. It is the wrong shape for two measured reasons and one structural one.

- **The secret would reach five jobs** — five runner images, five sets of installed packaging tools,
  five chances for a `set -x` in an unrelated step. One job that holds the key is one job to read
  when asking who could have used it.
- **`minisign` is not installable on all five.** jedisct1 publishes a Windows build for x86-64 and
  not for arm64, so the `windows-11-arm` leg would be signing through emulation or not at all. On
  one ubuntu runner the tool is an `apt-get install` away.
- The `build` job's five legs are about **producing** artifacts, and they are identical in shape
  today. Adding a step that is conditional on a secret and on the ref being a tag would make them
  five variations on a theme.

So `build` is untouched. A `release` job downloads all five uploads, signs everything once, and is
the only place the secret is named.

### D5 — `packaging/sign.sh`, and what it proves

```
packaging/sign.sh [--dist <dir>] [--key <file>] [--pubkey <base64>]
```

Defaults: `target/packaging/dist`, `~/.config/mixengine/updates.key`, and the key read out of
`crates/mixengine-core/src/updates.rs`. The password comes from `MIX_SIGN_PASSWORD` when it is set
and from the terminal when it is not, so the same script signs a release in CI and on the machine
that cut it.

`--pubkey` exists for D9's self-test and for nothing else: given one, the script verifies against
that key instead of the product's, and step 1 below — which is about the product's key — does not
run. A release never passes it, which is what makes the release path the one that has to agree with
the compiled-in constant.

**It signs prehashed, because that is what the product accepts.** Both shipped verifiers call
`minisign_verify`'s `verify(..., false)`, which refuses minisign's legacy algorithm. Measured on this
machine rather than read about: modern `minisign -S` writes the prehashed form by default — the
trusted comment ends in `hashed` — and `-l` is the opt-in to the legacy one. `-H` is a *verify* flag
meaning "require prehashed", and it is exactly `allow_legacy = false` spelled for the command line.
A legacy signature fails `minisign -V -H` with status 1, also measured.

The script therefore:

1. Reads `PUBLIC_KEY` out of the Rust source with `sed`, and compares it with `packaging/updates.pub`.
   Refuses if they differ. This is T79a's D3 — *prove the key chain, not merely the signature* — and
   it is here rather than only in the unit test so that a signing run by hand on a dirty tree cannot
   skip it.
2. Signs every file in the distribution directory that is not a `.sha256` and not a `.minisig`.
3. Verifies each one with `minisign -V -H -P "$pubkey"`. What that sentence means is *"MixEngine will
   accept this"* and not *"whatever secret this machine holds produced it"*. It is the only check in
   the whole design that can catch an Actions secret which is not the pair of the committed public
   key, and it catches it before anything is uploaded.
4. **Counts.** The number of `.minisig` files it wrote must equal the number of artifacts it found,
   and both are printed. A release with one unsigned artifact in it is the failure this script exists
   to prevent, and a glob that quietly matched nothing is how it would happen.

Secret handling: the key is written to a file under the runner's temporary directory with `umask
077` and removed by a `trap`, the password is piped on stdin (measured to work for both `-G` and
`-S`), and nothing in the script runs under `set -x`.

The default trusted comment is kept rather than replaced with `-t`. minisign already writes
`timestamp:… file:<name> hashed` into it, that comment is covered by the second signature, and a
custom one would be a second format to parse for facts the release page already states.

### D6 — A tag runs the whole workflow, and what it assembles is a draft

`ci.yml` gains one trigger: `push: tags: ['v*']`. Nothing else about the existing trigger philosophy
changes — `master` still fires by itself, every other branch still asks — and a tag joins the short
list of refs whose result nobody should have to remember to request. Running `lint`, `test`,
`system`, `bench` and `build` on the tag *is* item 1 of the release checklist, "all CI green", which
until now was a thing a person asserted.

**The release is created as a draft.** `gh release create --draft`, assets uploaded, and a person
clicks publish. Three reasons, and each on its own would be enough:

- T88's feed lives at `releases/latest/download/latest.json`, and a draft is not `latest`. The stable
  URL every installed copy polls must not move because somebody pushed a tag to see what would
  happen.
- **T86a has to watch a real download.** SmartScreen behaviour across two consecutive releases and
  the Gatekeeper flow on macOS 15 are measured on files a person downloads from a release page, and
  a release that publishes itself gives nobody a moment to be ready for that.
- Publishing to the world is an outward-facing, hard-to-reverse action. A tag push is a deliberate
  act; it is not the same deliberate act as "these bytes are now what everyone gets".

**`needs: [preflight, lint, test, build]`, and deliberately not `system` or `bench`.** Those two run
on the tag and a person reads them. They do not gate, because `bench` is bimodal on ubuntu — a
release blocked by somebody else's bad minute is a release process nobody trusts, and the failure it
would report is a wall-clock number rather than a wrong artifact. `system` is elevated and reaches
the machine's hosts file; it answers a question about the runner as much as about the code. What
gates a release is: the repository is consistent (`lint`), the code is correct on three operating
systems (`test`), and the artifacts were built (`build`).

The concurrency rule is amended so a tag run is never cancelled — `cancel-in-progress` becomes false
for `master` **and** for anything under `refs/tags/`. A cancelled release run leaves a half-uploaded
draft, which is the one state this design has no answer for.

### D7 — `preflight`, so a bad release fails in thirty seconds

Three questions can be answered without a compiler, and each of them turns an hour of runner time
into a red X you see before you have finished reading the tag name:

1. The tag is `v` + exactly the version in `[workspace.package]`. A release tagged `v0.2.0` whose
   binaries answer `0.1.0` is the classic mistake, and T88's post-restart version check exists
   because it happens.
2. `packaging/updates.pub` matches `core::updates::PUBLIC_KEY`. D2's unit test also says this, but it
   says it inside `test`, forty minutes later.
3. `UPDATE_SECRET_KEY` and `UPDATE_PASSWORD` are both non-empty. A missing secret is otherwise
   discovered by the job that runs after everything else.

It does not gate `build` — a conditional `needs` is not a thing GitHub Actions has, and adding one
would mean every ordinary run of the workflow waits behind a job that has nothing to say. It gates
`release`, and it fails visibly enough that the run can be cancelled by hand.

### D8 — Verified before the upload, and again after it

The script verifies what it signed. The job then downloads the assets back from the draft release and
runs `minisign -V -H` over every pair it finds there.

This is T79a's D5 and its reason holds unchanged: *created* and *published* are different claims, and
an upload that clobbered the wrong asset is exactly the accident the second check catches. It costs
one `gh release download` of a few dozen megabytes.

### D9 — The signing script is tested on every CI run, with a key nobody has to protect

`sign.sh` is a shell script that drives an external tool, and the only thing that would otherwise
exercise it is a release. So `lint` gains a step that:

1. generates a throwaway, **password-protected** keypair — password-protected on purpose, because the
   stdin path is the part of this design that a new minisign release could break;
2. builds a distribution directory of two junk files with `.sha256` companions;
3. runs `sign.sh` against them with the throwaway key, and asserts every artifact came out with a
   `.minisig` and no `.sha256` did;
4. runs it again with a *different* public key and asserts it **fails**. This is D5's step 3 being
   exercised — the check that would catch an Actions secret which is not the pair of the pinned
   public key, and the one check in this design that a test can prove works without holding the real
   secret.

It lives in `lint` rather than in `test` because it is a check on the repository and its tools, next
to `sqlx prepare --check` and the dependency-budget diff, and because `test` runs on three operating
systems with network egress blocked — installing minisign there would be three problems in exchange
for two answers nobody needs.

### D10 — What is signed, what is not, and what a `.sha256` is still for

Signed: every artifact in `target/packaging/dist` — the NSIS installer, the portable zip, the `.pkg`,
the `.deb`, the `.rpm`, the AppImage, and anything a later task adds beside them.

Not signed: the `.sha256` files. They stay, and `common.sh` already says what they are for — *a
person who downloaded twice and wants to know whether they got the same file*. A signature over a
checksum file would be a second, weaker way of saying what the signature over the artifact already
says.

Not here at all: `latest.json`. It is a document about payload archives that T88's D6 creates, and a
feed generated now would list five installers no updater can apply. Because the script signs a
*directory* rather than a list, the change T88 needs is a step that writes the file before
`sign.sh` runs.

### D11 — Rotating this key is a one-way door, and that is written down rather than engineered around

`PUBLIC_KEY` is compiled in. Every installed copy of MixEngine trusts exactly one key, so rotating it
means: every machine running a build from before the rotation can never verify a feed signed after
it. They do not fall back and they do not warn — T88's client keeps the last document it verified and
logs a refusal nobody reads. The fleet silently stops receiving updates, which is the one failure
mode an updater must not have.

Three responses were considered:

- **Accept a set of keys** — `PUBLIC_KEY` becomes `ACCEPTED_KEYS`, and a rotation publishes one
  release signed by both. It is the right answer to a compromise and it is unbuildable today, because
  the thing it protects (T88's client) does not exist and building the multi-key path before its only
  caller is the speculative work this project keeps refusing.
- **Sign the feed twice** — same shape, same objection, and it also doubles the wire format.
- **Write it down.** A rotation is an application release *and* an announcement, and until there is a
  reason to rotate there is nothing to build. `updates.md` gains the paragraph, so the day somebody
  needs to rotate they find the consequence before they find the command.

This is the third. The paragraph names the mitigation shape so that the task which needs it is a
decision already half made.

## Files

```
crates/mixengine-core/src/updates.rs        new — PUBLIC_KEY and the test that pins it
crates/mixengine-core/src/lib.rs            one `pub mod updates;`
packaging/updates.pub                       new — the committed public half
packaging/sign.sh                           new — sign a dist directory, and prove it
packaging/README.md                         "Nothing signs anything" stops being true
.github/workflows/ci.yml                    tag trigger, concurrency, preflight, release, self-test
.claude/features/updates.md                 the key exists; rotation is a one-way door
.claude/operations/build-and-release.md      the Signing section's left-hand column is built
.claude/roadmap/phase-9-ship.md             T86 ticked
docs/superpowers/specs/2026-09-04-t88-self-update-design.md   D2's ordering note
```

## What can go wrong, and what each thing does about it

| Failure | Where it is caught | What happens |
| --- | --- | --- |
| the Actions secret is not the pair of the committed public key | `sign.sh` step 3 | the run fails before anything is uploaded |
| `packaging/updates.pub` and `PUBLIC_KEY` have drifted | D2's unit test, and `preflight` | a red `test` job, and a red `preflight` thirty seconds into a release |
| the pinned key is a valid key, but the index's | D2's third assertion | a red `test` job |
| the tag says a version the binaries do not | `preflight` | the release job never starts |
| a secret is not set | `preflight` | the same, by name |
| a new artifact type is added and the glob misses it | `sign.sh` step 4's count | the run fails, naming both numbers |
| two build legs produce the same file name | the per-leg download, which refuses to overwrite | the run fails, naming the file |
| an upload clobbered the wrong asset | D8's re-download | the run fails with the draft still a draft |
| a signature was made in the legacy format | `minisign -V -H` | the run fails; the product would have refused the file |
| `minisign` is not in the runner's package repository | `mix_require` | the run fails naming the tool, rather than skipping the signing |
| a release run is cancelled half way | D6's concurrency rule | it is not cancelled |
| the key is lost | nothing | a new key, a new release, and D11's paragraph is what the person reads first |

## Testing

| What | Where | How |
| --- | --- | --- |
| the committed public key is the pinned one | `core` unit | `include_str!` and a string comparison |
| the pinned key parses | `core` unit | `minisign_verify::PublicKey::from_base64` |
| the pinned key is not one of the other two | `core` unit | three constants, two comparisons |
| the script signs every artifact and no metadata | `lint` step | a throwaway key and a fake dist |
| the script refuses a key that is not the pinned one | `lint` step | the same, with a second keypair |
| the password reaches minisign on stdin | `lint` step | the throwaway key is password-protected |
| the whole pipeline | the first tag | there is no way to test a release except by cutting one, which is why D6 makes the first one a draft |

Nothing here is `#[ignore]`d and nothing reaches the network: the unit tests read a committed file,
and the `lint` step signs files it made itself with a key it made itself.

## Operating it

**Generating the key** — once, on the machine that cuts releases, and never in CI:

```bash
minisign -G -p ~/.config/mixengine/updates.pub -s ~/.config/mixengine/updates.key
```

The private half stays in `~/.config/mixengine/`, outside every working tree, for the reason the
other two are there: one `git clean -fdx` in a checkout has already nearly taken a signing key with
it. Copy `updates.pub` to `packaging/updates.pub`, paste its second line into `core::updates::PUBLIC_KEY`,
and commit both in the same change — the unit test will not let them be committed apart.

**The Actions secrets**, on `mixnz/mixengine`, set by a person and never by a workflow:

| Secret | Contents |
| --- | --- |
| `UPDATE_SECRET_KEY` | the whole of `updates.key`, both lines |
| `UPDATE_PASSWORD` | the password that decrypts it |

**Cutting a release**: bump `[workspace.package] version`, update the changelog, push the tag. CI
runs everything, and the draft appears carrying every artifact the five build legs produced — eleven
of them today, two Windows rows of two, one `.pkg`, and two Linux rows of three — with a `.sha256`
and a `.minisig` beside each. Publishing is a click.

**Changing only the password** — `minisign -C -s ~/.config/mixengine/updates.key` — keeps the key
pair, so it changes one Actions secret and nothing in this repository. **Changing the key** is D11,
and is an application release.
