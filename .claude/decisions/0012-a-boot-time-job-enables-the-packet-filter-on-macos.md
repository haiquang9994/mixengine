# 0012. A boot-time job enables the packet filter on macOS

**Status**: Accepted
**Date**: 2026-08-23
**Qualifies** the "no persistent root process, ever" rule in [../../CLAUDE.md](../../CLAUDE.md) and
in [0005](0005-on-demand-elevation.md), which otherwise stands unchanged.

## Context

`http://blog.test` needs port 80. macOS reserves every port below 1024 for root, and the only
mechanism that system has for handing one to an unprivileged program is a packet-filter redirect:
`net.inet.ip.portrange.reservedhigh` — the knob FreeBSD has and the obvious first candidate — **does
not exist on macOS 15**, measured on a real runner in
[run 32620072917](https://github.com/mixnz/mixengine/actions/runs/32620072917). So that system
has pf or it has nothing, and there is no fallback chain to design.

pf itself is **disabled on every boot**, and Apple's own `/etc/pf.conf` says it is meant to stay that
way:

> each component which utilizes PF is responsible for enabling and disabling PF via -E and -X as
> documented in pfctl(8)

`pfctl -e` needs root. A redirect that is only *installed* therefore works until the first reboot and
then silently stops — leaving a front end answering on 8080 that nothing reaches on 80, with no
error anywhere and nothing for the user to look at.

## Decision

**MixEngine installs a LaunchDaemon that enables the packet filter at boot.**

`/Library/LaunchDaemons/dev.mixengine.pf.plist`, root-owned, with `RunAtLoad` true, `KeepAlive`
**false**, and one fixed command that exits:

```
/sbin/pfctl -e -f /etc/pf.conf
```

It is written by `mixengine-elevate` under the same prompt as the anchor and the `/etc/pf.conf`
block, and removed by the same revoke. The whole plist is rendered inside the helper from constants:
no part of it comes from the request.

### The rule it bends, named

[`CLAUDE.md`](../../CLAUDE.md) says *no persistent root process, ever*. This is not a process that
persists — `KeepAlive` is false and the command exits in milliseconds. It is a **standing ability to
run one fixed command as root at boot**, which is a smaller thing than a daemon and a larger thing
than nothing. It gets a record rather than a comment because it is the first thing MixEngine installs
that acts without anybody asking.

### What bounds it

- The plist is **root-owned and root-written**. A compromised daemon cannot edit it; changing what
  runs at boot needs a second prompt, which is exactly the control [ADR 0005](0005-on-demand-elevation.md)
  relies on everywhere else.
- The command takes **no argument from anywhere** — not from the request, not from the environment,
  not from a file the user owns.
- What it does is enable a firewall whose rules are somebody else's file. It loads `/etc/pf.conf`,
  which is Apple's, plus whatever MixEngine's marked block in it declares.

## Consequences

- macOS is the one system where MixEngine leaves something behind that runs at boot. Uninstall
  (**T87**) must remove it, and `mix doctor` (**T47**) must report it.
- The probe cannot ask pf anything: `/dev/pf` belongs to root and the daemon runs as the user. What
  it reads instead is the three files, compared against what a grant would write. That proves the
  configuration is in place; the plist is what makes it true again after a reboot.
- Two accounts on one machine share the one job, and the one anchor with it — the same debt
  [T41](../roadmap/phase-4-sites-and-elevation.md) recorded for the hosts file, for the same reason:
  the artifact is machine-wide and the state it is generated from is per-home.
- The revoke deliberately does **not** run `pfctl -d`. By then there is no way to know who else has
  come to depend on pf being up, and pf enabled with none of our rules in it is not observably
  different from pf disabled.

## Alternatives considered

**A prompt on every boot.** [ADR 0005](0005-on-demand-elevation.md) budgets about two prompts for the
product's whole lifetime. This is one per day per machine, and it arrives before the user has asked
for anything.

**A LaunchAgent at user level.** `pfctl -e` needs root, so an agent running as the user cannot run
it. The mechanism does not exist.

**Not enabling pf at all, and telling the user to run `sudo pfctl -e`.** A product whose central
promise needs a terminal command after every reboot has not delivered that promise.

**Binding 8080 and telling people to type it.** `http://blog.test:8080` is not `http://blog.test`,
which is the whole feature.
