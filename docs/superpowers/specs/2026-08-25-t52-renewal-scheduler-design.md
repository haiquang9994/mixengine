# T52 — renewal scheduler

**Roadmap task:** T52, phase 5. **Depends on:** T50 (leaf issuance), T51 (the front end serves the
leaf), T43 (the site generator).

A leaf lives 90 days. Nothing in this daemon has ever renewed one while it was running: the only
thing that reissues an expiring certificate today is a daemon **start**. A machine whose daemon
stays up for three months therefore reaches a red padlock with no warning and no defect anywhere in
the code — every part behaved exactly as written.

This task is the part that wakes up.

Two of the three things the roadmap line names are already built, and finding that out is what makes
the third one small. What it also finds is a type from T50 that gives two different answers the same
name, which T52 is the first caller that has to tell apart.

---

## D1 — the on-boot check and the 30-day threshold already exist

**Measured against the code rather than assumed from the roadmap line.**

*"Plus a check on daemon start"* — `crates/mixengine-daemon/src/main.rs` calls
`Certificates::issue(None)` on every start, `leaf::ensure` refuses to reuse a leaf with 30 days or
fewer remaining, and the generator blocks run after it in the start's own ordering. Restarting the
daemon today renews and reloads. That half is discharged, and T52 owes it a **test** rather than an
edit — there is nothing asserting the ordering that makes it work.

*"< 30 days threshold"* — `leaf::RENEW_WITHIN_DAYS` is that number, and `mix doctor`'s
`SiteCertificateMissing` check already treats a certificate with 30 days or fewer left as a site
without one, repairable by `IssueCertificates`. So the standing report of an expiring certificate
exists too.

What is missing is exactly three things:

1. **A tick.** Nothing wakes a running daemon up to ask the question.
2. **A reload for a renewal nobody asked for.** `site.create`, `site.update` and the start each run
   the generator themselves after issuing. A renewal driven by a clock has no such caller, so it has
   to run the generator itself or the new certificate is written and never served.
3. **`CertExpiring`.** `crates/mixengine-proto/src/event.rs` names this variant in its module
   documentation as one to be declared *"one variant at a time, as the code that emits it lands"*.
   T52 is that landing.

## D2 — the check runs hourly, and the 30-day margin is why it may be imprecise

`.claude/features/tls.md` says *"a daily scheduler task"*. Taken literally that is a 24-hour
`tokio::time::interval`, and a 24-hour interval on a laptop is not 24 hours: Tokio measures from
`std::time::Instant`, which is `CLOCK_MONOTONIC` on Linux and `mach_absolute_time` on macOS, and
neither advances while the machine is suspended. A laptop closed over a weekend counts none of it.
The alarm set for tomorrow rings on Tuesday.

Windows counts it differently again, and **this design does not depend on knowing which way** —
that is the point rather than an omission. A scheduler whose correctness rests on whether a
particular platform's clock counts suspended time is a scheduler with three behaviours, in a
workspace whose rule is cross-platform or not merged. Anything asserted here about Windows'
performance counter would be a claim nothing in this repository measures.

Two ways to fix a drifting alarm: make the alarm accurate, or make the drift not matter.

**The drift is made not to matter, because it already does not.** The question a tick asks is *"does
any leaf have 30 days or fewer left"* — and the answer to that does not depend on how often it is
asked. A leaf is renewed a full month before anything breaks, which means the 30-day threshold **is
the tolerance** for the scheduler's imprecision. A scheduler that fires four days late is a
scheduler that renews with 26 days to spare.

So the tick does not need to be accurate. It needs to be **cheap**, which it is: one pass reads a
handful of PEM files and compares dates. Hourly, and nothing is written unless something is due.

This also settles what happens on resume from suspend, which would otherwise need its own mechanism:
nothing, because the next tick is at most an hour away and no OS notification is involved. And it
sidesteps `MissedTickBehavior` entirely — an interval that fires late has nothing to catch up on,
because a pass that finds nothing due does nothing.

**Rejected: recording the wall-clock time of the last check.** It is the accurate answer, it detects
a clock that jumped, and it costs a piece of persisted state that has to be right — written where,
read when, and wrong in which direction if the file is lost. Against a threshold thirty days wide,
that is bookkeeping bought for nothing.

## D3 — the period is a setting, not a constant

**T51's lesson, applied one task later.** The nginx TLS port was written as a constant that no test
could move, and the real-nginx suite could not bind 443 — the constant is what made the test
impossible, and the missing test is what nearly let the whole configuration be refused on a real
machine. A period no test can move is the same shape: the loop would be the one part of this task
nothing ever runs.

A new `[certs]` section beside `[dns]`, one key:

```toml
[certs]
renew_check_seconds = 3600
```

Refused at zero, which is a busy loop rather than a setting. No ceiling: a period longer than sixty
days would be the first one that could miss a window, and a person who writes that number has
answered a different question than this key asks.

**Seconds and not milliseconds**, so the integration test sets `1` and waits rather than asking the
configuration file to carry a unit no user would ever want. A one-second period in a test is honest
about what it proves: the loop really runs on its own timer.

## D4 — `Refused` is two different answers under one name, and T52 is the first caller that must tell them apart

T50's `IssueOutcome::Refused` documents its own reasons as *"no usable authority, HTTPS not declared,
no domains"*. The middle one is not a refusal. A site that declares no HTTPS did not ask for a
certificate, and reporting that as a refusal says something failed when nothing did.

Nothing needed the distinction before, because both callers were about to log the same sentence. T52
needs it: a renewal loop that treats every `Refused` as a failure would announce *"this certificate
is expiring and could not be renewed"* for every plaintext site on the machine, once an hour,
forever.

**And the conflation is already producing one wrong line today.** `Sites::now_has_a_certificate`
warns on every `Refused`, so creating a site with HTTPS off logs `the site has no certificate yet`
for a site that never wanted one. That is read from the code rather than from a test, so T52 proves
it the other way round: a test that creates a plaintext site and asserts that warning is **absent**.

So `IssueOutcome` gains a fourth variant:

```rust
/// Nothing was written because nothing was asked for: this site declares no HTTPS.
///
/// **Not a refusal.** A refusal is MixEngine failing to do something wanted; this is MixEngine
/// correctly doing nothing.
NotWanted { because: String },
```

`no domains` stays a `Refused` — a site with no domains is a row that should not exist, and calling
it "not wanted" would hide it.

**Rejected: leaving the wire type alone and filtering inside the renewal loop.** The loop would
fetch the site rows itself and keep only those with `https_enabled`, which is a second copy of the
rule `issued()` already owns. `doctor.rs` states the cost of exactly that in a comment about
`ensure`'s fourth question: two copies of one rule to keep in step, and the copy that drifts reports
a machine as faulty for a certificate the repair then declines to replace.

## D5 — a pass reports; the loop acts

The renewal component splits in two, and the split is drawn where testing needs it:

```rust
/// One site whose certificate is running out and could not be replaced.
struct Failure {
    domain: String,
    because: String,
}

/// What one pass over this home's certificates did.
enum Pass {
    /// Nothing was read and nothing was written, and this is why.
    Skipped { because: String },

    /// The pass ran.
    Ran { renewed: usize, failed: Vec<Failure> },
}

async fn once(&self) -> Pass;
```

**`Skipped` is a variant rather than an empty `Ran`**, and that is what makes D6's gate testable at
all. A pass that stopped because there is no authority and a pass that ran and found nothing due
would otherwise be the same value — so the test for the gate would pass whether or not the gate had
been written, which is the definition of a test that proves nothing.

`once` reads and issues. It does **not** reload, and does not emit. The loop reads the `Pass` and
decides: `renewed > 0` means call `Registry::reconfigure`; `failed` goes through D7's filter to the
event stream.

The reason is that `once` needs only a `Certificates`, so a unit test can build one over a temporary
directory — which is how `crates/mixengine-daemon/src/certs.rs`'s own tests already work. A `once`
that reloaded would need a whole `Registry`, a store, a supervisor and a front end before it could
be asked the simplest question in this task.

**The reload mechanism is T51's and is not touched.** `reconfigure` renders every site file; a
renewed certificate has a new fingerprint; the fingerprint is in the file's header; `document::install`
compares bytes, finds a difference, installs and reloads. T52 adds no reload path of its own, and if
it ever looks like it needs one, that is the signal that something in T51 was undone.

**A `reconfigure` that fails is logged, not announced.** Nothing was installed — `document::install`
stages first — so the front end is still serving the previous configuration with the previous
certificate, which is exactly the state `mix doctor`'s `GeneratedConfigStale` exists to report.

## D6 — a home with no usable authority does nothing at all

Before issuing anything, the pass reads the authority. `CaState::Absent` or `CaState::Unusable`
means the pass ends there.

This is `mix doctor`'s own behaviour and deliberately the same words: its site-certificate check is
`Skipped` with *"this home has no usable certificate authority to sign with — `mix cert ca-status`
says which"* rather than reporting every site as broken. One damaged authority is one problem, and
announcing it once per site would bury the single line that says what to fix.

The authority is read with `certs::ca::read` rather than `Certificates::status`, because `status`
also asks the machine's trust stores and its browser databases — which on Linux spawns `certutil`
once per profile. That is a reasonable price on a start and an unreasonable one every hour.

## D7 — `CertExpiring` announces a change, never a heartbeat

The event stream holds 1024 messages for the whole daemon, and `event.rs` states the rule plainly:
a producer reports a change and not a heartbeat, an enqueue that changed nothing publishes nothing.
A renewal that fails will keep failing every hour — a disk that is full at nine is full at ten — so
an event per failing pass would spend a client's entire allowance restating one fact.

So the loop keeps the set of domains it has already announced, and the diff is a free function
rather than something buried in the loop body — a rule about what is *not* sent is worth being able
to test directly:

```rust
/// The failures worth announcing, and the set updated to match.
fn newly(announced: &mut BTreeSet<String>, failed: &[Failure]) -> Vec<Failure>;
```

- a domain that has just **entered** the failing set is announced,
- a domain already in it is silent,
- a domain that **leaves** it — the renewal worked — is removed from the set, so that a later
  failure is announced again rather than swallowed.

The variant carries the site and the reason, and nothing else:

```rust
/// This site's certificate is close to expiry and MixEngine could not renew it — task **T52**.
CertExpiring {
    /// The site, by its primary domain.
    domain: String,
    /// Why the renewal did not happen, in words.
    because: String,
},
```

No `days_left`. Events are best-effort and never the only way state is learned — the client that
wants the number calls `cert.status` or runs `mix doctor`, both of which already report it, and a
number that travelled on an event would be a second copy that is stale by definition.

**The set lives in the loop and not on disk.** A restart re-announces, which is correct: a restart
is also a fresh attempt, and a client that just connected has been told nothing yet.

## D8 — what has to be true, and how it is proved

**Unit, in the daemon crate:**

- the gate: a home whose authority is absent gives `Pass::Skipped`, which is a different value from
  a pass that ran and found nothing;
- the report becomes a pass correctly, over a `CertIssueReport` built by hand rather than by
  issuing: `Issued` counts as renewed, `Reused` does not, `Refused` is a failure carrying its
  domain, and **`NotWanted` is not a failure** — the last is the assertion that keeps a plaintext
  site off the event stream;
- the announcement filter: a domain entering the failing set is announced once, the second pass
  announces nothing, and a domain that recovers and then fails again is announced again.

**Not a unit test asserting that a certificate 70 days old is reissued.** T50 already owns that
claim in `leaf.rs`, as `a_certificate_is_reissued_once_it_is_inside_the_renewal_window`, and a
second copy here would be a test of somebody else's rule that goes red for a reason its name does
not mention.

**Integration, in a new `crates/mixengine-daemon/tests/renewal.rs`**, against a real daemon with
`renew_check_seconds = 1`: create an HTTPS site over the socket the way `tests/sites.rs` does,
rewrite its certificate as one issued 70 days ago, and assert that within a few seconds **the
certificate on disk has been replaced and the daemon has regenerated afterwards**. The second half
is the one that matters: a renewal wired only to the certificate directory would pass the first
assertion and leave the front end serving the certificate it already holds.

**Not in `crates/mixengine-cli/tests/cert.rs`**, where a suite about certificates would otherwise
belong. Writing a backdated certificate needs `mixengine_core::certs::leaf::ensure`, and
`mixengine-proto/tests/workspace_layering.rs` forbids `mixengine-cli` from depending on
`mixengine-core` at all — a rule about the shipped binary that a dev-dependency would work around
rather than respect. The daemon crate already depends on core, and this is daemon behaviour.

**The testkit gains one thing**: a way for a test to add configuration. `Home::new` writes exactly
`[dns] port = 0` today and nothing can add to it, which its own `SEEDED` comment anticipates as
*"a future seed"*. This is that seed.

**And the on-boot test D1 asks for**: a daemon started on a home that already holds an HTTPS site
whose certificate is missing has issued one by the time it answers. Nothing asserts that today — the
suites that exercise issuance all create the site through the API, which issues on its own path —
so the half of T52 that already works is the half with no test behind it.

What that test does **not** claim is the ordering against the generator, which needs a front end
present to be visible at all. It is stated in the start's own comments and relied on by T51; making
it an assertion means installing a real Caddy, and the CLI's front-end suites are where that would
belong if it is ever worth the minutes.

## D9 — what this deliberately does not do

**The authority is not renewed.** It lives ten years, and replacing it invalidates every leaf and
every trust store holding it — that is `cert.ca_rotate`, T54, and it is a destructive operation with
a person on the other end of it, not something a timer does at three in the morning.

**Trust stores and browser databases are not re-checked.** The start does that, once, where a failure
can still be batched into the single elevation prompt. An hourly loop that re-read them would spawn
`certutil` per profile on Linux forever, to answer a question whose answer changes when a person
changes it.

**Nothing is deleted.** A leaf whose site was renamed or removed stays on disk. Removal is T54's and
T87's, on the reasoning T42's D12 and T45's D13 set and T50 and T51 followed — for the fifth time.

**No handshake.** Whether the running server actually presents the renewed certificate is `mix cert
status`, T53. What is proved here is that the file changed and the configuration that names it
changed with it.

**No backoff and no jitter.** One machine, one home, one loop: there is nothing to stampede, and a
retry an hour later is already the gentlest schedule this task has.

**No `cert.renew` method.** `cert.issue` already reissues anything within the threshold, and a
second method meaning "the same thing but on purpose" would be two names for one operation. The
`force` a person might want out of one is T53's, next to the handshake that would tell them why.
