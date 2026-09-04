# 0017. A machine with Smart App Control enforcing is a configuration MixEngine does not support

**Status**: Accepted
**Date**: 2026-09-04

## Context

Smart App Control is not SmartScreen with a harder voice. It is a WDAC policy enforced by Code
Integrity at *image load*: no warning, no "Run anyway", no per-path exclusion, and Microsoft Defender
exclusions do not apply to it, because they configure a different subsystem. A refusal reaches the
caller as `os error 4551`, *"An Application Control policy has blocked this file"*, and the events
behind it are `Microsoft-Windows-CodeIntegrity/Operational` 3033, 3077 and 3118. All of that was
measured on a developer machine on 2026-08-13 and is recorded in
[../features/updates.md](../features/updates.md).

Every binary MixEngine ships is unsigned, by
[ADR 0005](0005-on-demand-elevation.md). **T94** asked the question that follows from those two
sentences together: does a certificate this project can buy repair it?

Three facts decide the answer, and none of them costs anything to establish:

1. **A certificate covers four images.** `mix.exe`, `mixengined.exe`, `mixengine-elevate.exe` and
   `mixengine-shim.exe` are the whole of what MixEngine builds. T86a's **W1** measures all of them,
   and `setup.exe`, as `NotSigned` today.
2. **It covers nothing else that a MixEngine install runs.** T20a and T27 measured every upstream
   Windows artifact this project redistributes: `php.exe` and the DLLs beside it, `nginx.exe`,
   `caddy.exe`, `python.exe`, `ruby.exe` are all unsigned upstream. **Node is the only signed one.**
   Add `mariadbd.exe`, `postgres.exe`, `memcached.exe` and a Redis-compatible server and the shape
   does not change. [runtime-packaging.md](../operations/runtime-packaging.md) already put the
   consequence plainly: SAC would refuse the same artifacts even if MixEngine shipped none of its
   own.
3. **The judgement is on the file, not on the process tree.** A signed `mixengined.exe` starting an
   unsigned `caddy.exe` lends it nothing; the second load is judged on its own.

So a certificate repairs the *first* image load and the product dies at the second. Whether an EV
certificate is honoured by SAC immediately — the thing T41a proposed to settle by buying the cheapest
usable one — turns out not to matter: it is a question about the four images that were never the
ones deciding the outcome. SAC has no publisher allow-list to buy a place in; it admits a file on a
signature the policy already trusts or on ISG reputation, and reputation is the only mechanism a
certificate improves.

## Decision

**A Windows machine with Smart App Control enforcing is a configuration MixEngine does not support.
It is detected, it is named where a program will not load, and it is not worked around.**

Three parts, all built by T94:

1. **`mix doctor` reports it.** A platform capability reads
   `HKLM\SYSTEM\CurrentControlSet\Control\CI\Policy\VerifiedAndReputablePolicyState`; enforcing is a
   `Problem` carrying `ProblemId::ApplicationControlEnforced`, evaluating is a `Note`, and macOS and
   Linux are a `Skipped` that says why.
2. **A refused image load says so in words.** Where MixEngine loads a program it did not build — the
   post-install smoke test, and a supervised service's spawn — `4551` is turned into a sentence
   naming *an application control policy* rather than a number nobody would recognise.
3. **`daemon.doctor_repair` declines it out loud.** `Planned::Untouched`, beside the two other
   conditions this machine did to itself: turning Smart App Control off cannot be undone without
   reinstalling Windows.

**MixEngine does not ask a user to turn Smart App Control off.** That is the load-bearing half of
this decision, and it is what makes the rest of it honest.

## Consequences

**Easy:**

- Nothing is built to work around a policy that has no override to work around.
- The condition has one name, one report and one sentence, so it cannot be diagnosed three ways.
- A user on such a machine is told what is wrong on the first run of `mix doctor`, rather than
  meeting `os error 4551` on their third attempt at `mix install php`.

**Hard, and accepted:**

- **There is a machine MixEngine simply does not run on**, and on the harshest version of it —
  where `mixengined.exe` itself is refused — none of the reporting above can run either. The only
  record there is Windows' own Code Integrity log, and that is written into
  [../features/updates.md](../features/updates.md) as the diagnosis of last resort.
- **`mix doctor` reports a problem on an enforcing machine where everything currently works.** That
  is deliberate: the next image MixEngine loads is a runtime archive whose hash has never existed
  anywhere, which is exactly the first-seen case a refusal was measured for.
- **The size of the affected population is not known, and this decision does not rest on it.** Both
  alternatives below are refused at every size, so a number would change nothing; and nobody here can
  take it — there is no telemetry, and T91's crash reporting is opt-in and is not an inventory of
  machines. What is reasoned rather than measured: SAC ships enabled on clean Windows 11 installs,
  stays off after an in-place upgrade, and takes itself out of evaluation mode when it observes
  development activity, which is a description of MixEngine's own audience.

**What would reopen this**, so a future reader does not have to reconstruct it:

- A Windows in which Smart App Control accepts a publisher allow-list, or any mechanism by which a
  purchased certificate admits a file rather than only accruing reputation for it.
- An upstream supply in which PHP, nginx, Caddy and the database servers arrive signed. The
  arithmetic here is entirely about the *uncovered* set; if that set empties, the question is a
  different one.

## This does not supersede ADR 0005 — it confirms it

The roadmap predicted that a bad answer here would **supersede**
[ADR 0005](0005-on-demand-elevation.md), on the grounds that *"no OS code signing"* would have stopped
being a trade of first-launch friendliness against a few hundred dollars a year.

It has not stopped being that trade, and fact 2 above is why. ADR 0005 declined to buy certificates
for MixEngine's own binaries. Buying them would not have produced a product that runs under Smart App
Control, because the binaries that decide the outcome are not ours to sign. **The certificate was
never the thing standing between this product and this policy**, so 0005's cost-benefit is untouched
and what is added here is a case it never addressed: what happens on a machine that enforces.

The residual [security-model.md](../architecture/security-model.md) records — that malware replacing
`mixengine-elevate` before first run gets root at the next prompt, and that only a signature the OS
checks before the prompt would close it — is likewise **not** closed by this ADR. T94 answered
whether a certificate repairs Smart App Control. It did not answer whether one would be worth buying
for that hole, which remains open.

## Alternatives considered

- **Rebuild and sign every runtime.** The only remedy that would actually make MixEngine work under
  an enforcing SAC. Rejected: it means owning a build pipeline for PHP and its extensions, nginx,
  MariaDB, PostgreSQL, Redis, Ruby and Python, on two Windows architectures, for as long as the
  product exists — including their security updates. This is precisely the maintenance cost
  *"borrow before you build"* declined in [runtime-packaging.md](../operations/runtime-packaging.md),
  and signing does not reduce it by a line. It would also have to be re-argued rather than assumed,
  which is what this entry is.
- **Ask the user to turn Smart App Control off.** Rejected on its own terms rather than on cost:
  Smart App Control cannot be re-enabled without reinstalling Windows, so this is asking somebody to
  permanently lower their machine's defences in order to run a development tool. A product that
  requires that is, in another phrasing, a product that does not start.
- **Buy a certificate and hope.** Rejected because fact 2 makes it measurable in advance: it would
  move four images out of the uncovered set and leave every runtime in it. That is not a fix bought
  at a price; it is a smaller version of the same failure.
- **Say nothing and let `4551` reach the user.** Rejected. It is the status quo, and it sends
  somebody to Defender's exclusion list — which does not apply — instead of telling them the one true
  thing: this machine will not run this software.
