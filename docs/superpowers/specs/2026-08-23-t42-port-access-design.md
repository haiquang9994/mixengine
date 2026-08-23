# T42 — `PortAccess`: being allowed to answer on 80 and 443

**Roadmap:** T42, `.claude/roadmap/phase-4-sites-and-elevation.md`
**Depends on:** T40 (the helper and its file protocol), T40b (the queue), T64 (`mix elevation grant`),
T41 (the marker-block engine and the first producer), T37 (`core::services::front_end`)

## What this closes

`http://blog.test` is the product's promise, and a browser asking for it asks for port 80. On two of
the three systems an unprivileged process may not have it. Today nothing in this workspace binds 80
at all — both front-end recipes deliberately render a configuration that listens on nothing a site
would be reached on, each with a comment naming this task as the reason.

T42 does not make a site answer. It makes the machine *allow* the answer, and it tells the layer
above which port a program must actually bind in order for 80 to reach it. T43 is what puts a site
behind that.

## What was measured, rather than remembered

The three facts this design forks on were measured on GitHub runners in
[run 32620072917](https://github.com/haiquang9994/mixengine/actions/runs/32620072917), from a
throwaway workflow on a branch that no longer exists. A Windows developer machine can answer none of
them.

**macOS — pf redirects loopback, which was the thing in doubt.** A `rdr pass on lo0 inet proto tcp
from any to 127.0.0.1 port 80 -> 127.0.0.1 port 8080` rule was loaded, a trivial HTTP server put on
8080, and `curl http://127.0.0.1/` returned that server's page. `curl http://localhost/` did too. So
did the same rule loaded through an anchor declared in `/etc/pf.conf`, and the anchor's contents
survived a bare `pfctl -f /etc/pf.conf` reload. pf translation is documented against *inbound*
packets and locally-originated loopback traffic is exactly where that reasoning is usually said to
stop holding; it holds.

**macOS — `::1` is not redirected.** The rule is `inet`. See D6.

**macOS — the fallback named in the roadmap does not exist.**
`sysctl net.inet.ip.portrange.reservedhigh` answers `unknown oid` on macOS 15. pf is the only
mechanism, which is why D2 has no second branch.

**macOS — pf is off, and Apple says it stays off.** `pfctl -s info` on an untouched runner reports
`Status: Disabled`, and the shipped `/etc/pf.conf` says so itself: *"PF will not be automatically
enabled, however. Instead, each component which utilizes PF is responsible for enabling and
disabling."* Apple's own boot job loads `/etc/pf.conf` and does not enable pf. **This half is read
rather than measured** — a hosted runner cannot be rebooted — and it is the whole reason for D3 and
for ADR 0012.

**Linux — `setcap` works, and its loss is detectable without any privilege.** A binary under `$HOME`
on ext4 was refused port 80, granted `cap_net_bind_service=+ep`, and then bound it. Copying a fresh
build over the file emptied the capability and the bind was refused again. Renaming a staged file
into place — which is how T21 installs — emptied it too. And `getcap` run as the ordinary user read
the capability back in full. That last fact is what makes D7 possible: the daemon can answer "is the
grant still there?" on every start, for free, with no prompt and no elevation.

## What already exists, and is reused unchanged

- **The queue and its guarded upsert** (T40b, T41 D2): `Elevation::enqueue`, the `dedupe_key` unique
  index, `ON CONFLICT … DO UPDATE … WHERE op <> excluded.op`, the `ElevationRequired` event and the
  degraded mode a decline leaves behind. T42 adds a producer, not a mechanism.
- **`mix elevation grant`** (T64) prints every pending operation and what it will literally change,
  and refuses to raise anything it cannot be answered about. T42 owes it a description per operation
  and nothing else.
- **The helper's request checks** (T40): the request file must not be a symlink, must be a regular
  file, must not belong to a superuser, and must not be writable by anyone but its owner —
  `elevated::owner_of` and `elevated::others_can_write`. D5 applies the last two to a second file.
- **The atomic replace** (T41 D7): temp file in the same directory, `sync_all`, `ReplaceFileW` on
  Windows and `rename` plus a directory fsync on Unix, carrying the old file's ownership and mode.
- **`core::services::front_end::held_by`** (T37) answers which row is the home's front end, by the
  recipe's `Role` rather than by name.
- **`ServiceSpec::program()`** is the absolute path of the program a row runs.

## Decisions

### D1 — The capability reads; it never prompts and never writes

`PortAccess` on `Host` answers two questions and mutates nothing, which is what lets the daemon call
it on every start and from an error path.

The second question is the one that leaves the platform layer. On macOS a front end must bind 8080
to answer on 80; on the other two it binds 80. That difference has to reach the configuration
generator in `mixengine-core`, and `#[cfg]` in core is forbidden. So the capability returns the
mapping as data and core asks for it, exactly as `PortOwner` returns a `PortHolder` rather than
having each caller ask the OS.

`PortAccess` is a new file under `traits/`, not an addition to `traits/ports.rs`. That file's own
doc comment already draws the line: `PortOwner` is about who got to a port first, this is about
being allowed to bind one at all.

### D2 — One mechanism per OS, chosen by the OS, never negotiated

| | Method | What is granted | What the front end binds |
| --- | --- | --- | --- |
| Windows | `Direct` | nothing; there are no privileged ports | 80, 443 |
| Linux | `Capability` | `cap_net_bind_service` on the front-end binary | 80, 443 |
| macOS | `Redirect` | a pf anchor, plus what D3 adds | 8080, 8443 |

There is no runtime negotiation and no fallback chain, because the measurement removed the only
candidate for one: macOS has pf or it has nothing. A machine where the chosen mechanism cannot be
applied gets `Error::UnsupportedPlatform` with the manual workaround in its reason, per the platform
layer's rule 4.

### D3 — On macOS the grant is three artifacts, and one of them is a boot-time root job

pf is disabled on every boot and `pfctl -e` needs root, so a redirect that is only *installed* is a
redirect that works until the first reboot and then silently stops — leaving a front end answering
on 8080 that nothing reaches on 80. Something must enable pf at boot, as root, without a prompt.

The grant therefore writes three root-owned things:

1. `/etc/pf.anchors/mixengine` — the rendered rules.
2. A marker block in `/etc/pf.conf` carrying `rdr-anchor "mixengine"` and
   `load anchor "mixengine" from "/etc/pf.anchors/mixengine"`. pf.conf is order-sensitive —
   translation rules must precede filter rules — so the block is spliced immediately after Apple's
   own `rdr-anchor` line rather than appended.
3. `/Library/LaunchDaemons/dev.mixengine.pf.plist`, `RunAtLoad`, running `/sbin/pfctl -e -f
   /etc/pf.conf` and exiting.

The third contradicts a written non-negotiable — *no persistent root process, ever* — closely enough
to need saying properly, so it lands with **ADR 0012**. The short form: it is not a process that
persists but a standing ability to run one fixed command as root at boot; the plist and the anchor
are both root-owned, so a compromised daemon can change neither; and the alternative measured against
it is a prompt on every boot, against an ADR 0005 budget of about two prompts for the product's whole
lifetime.

`Revoke` removes all three and **does not run `pfctl -d`**. By then there is no way to know who else
has come to depend on pf being up, and pf enabled with none of our rules in it is not observably
different from pf disabled.

### D4 — The operation carries whole state, in a plan the helper can validate field by field

```rust
PortAccessGrant { plan: PortAccessPlan },
PortAccessRevoke { target: PortAccessTarget },

pub enum PortAccessPlan {
    Capability { binary: PathBuf, ports: Vec<u16> },
    Redirect { redirects: Vec<PortRedirect> },
}

pub enum PortAccessTarget {
    Capability { binary: PathBuf },
    Redirect {},
}

pub struct PortRedirect { pub answer: u16, pub bind: u16 }
```

**Revoke names its target, and a bare `PortAccessRevoke {}` was wrong.** A Linux capability lives on
a file, so taking it back means naming that file, and a helper that is handed nothing has nothing to
clear — the grant's path is the daemon's knowledge and the helper keeps no state between runs.
Reusing `PortAccessPlan` would have carried `ports` into an operation that does not read them, which
is the field D4 exists to forbid; so revoke gets the same two-variant shape with only the field it
uses. Each OS refuses the variant that is not its mechanism, exactly as it does for the grant.

Whole state rather than a delta, for T41 D1's reason: `AlreadyDone` becomes a byte comparison and a
superseded request is a replaced row rather than a second prompt.

Two variants rather than one struct holding both a binary and a redirect list, because **a field the
helper does not use is a field the helper cannot validate**, and this binary's entire job is to
validate. On Windows both variants are refused as `Unsupported`; on Linux `Redirect` is, and on
macOS `Capability` is. No branch quietly does nothing.

The redirect targets are 8080 and 8443, fixed. A program already holding 8080 makes the front end
fail to bind, which is the failure T38 already diagnoses by name. Choosing the target through the
port allocator instead would need no change to this wire type, so it is not designed for now.

### D5 — What the helper checks, per variant

`Capability`:

- every port in `ports` is in `{80, 443}` — the recorded allowlist the security model requires;
- `binary` is not a symlink, and is a regular file, read through `symlink_metadata`;
- `binary` is **owned by the caller** and **not writable by anyone else** — the same two checks
  `request.rs` already applies to the request file, through the same two functions.

Ownership by the caller is the strongest assertion available. The helper cannot be told where
`MIXENGINE_HOME` is, because the daemon is what would be telling it and the daemon is the thing being
guarded against; but the filesystem already knows who wrote the file, and the request's own identity
is established the same way.

`Redirect`:

- every `answer` is in `{80, 443}`;
- every `bind` is at least 1024;
- no `answer` appears twice.

The helper renders the anchor text itself from those numbers. **It never accepts text.**

`PortAccessTarget::Capability` runs the same three checks on its `binary` and nothing else — there is
no port to bound, because clearing the attribute clears all of it. `PortAccessTarget::Redirect {}`
carries nothing and so validates nothing; what bounds it is that the three paths it removes are
constants in this binary.

Both write atomically under an advisory lock and append one audit line, as T41 does.

### D6 — IPv4 only

The measured rule is `inet`, and `::1` was not redirected. That matches T41 D5, where a managed
domain resolves to `127.0.0.1` and nothing else, and it matches what the internal DNS server will
answer. An `inet6` rule is one more line in the anchor if a reason for it ever appears; there is none
today, and a rule nobody needs is a rule nobody tests.

### D7 — The producer is the re-probe, and they are the same thing

On every daemon start, after registry recovery: find the front end, take its `program()`, probe, and
enqueue `PortAccessGrant` only when the probe says the grant is absent. No front end means nothing is
asked for.

The roadmap states the requirement as "re-probe after every app update, because setcap is lost when
the binary is replaced". Probing on every start covers that and more — it also catches a capability
lost to something that was not an update, and it needs no hook into the updater. The measurement
above is what makes it affordable: reading the capability back costs one `getxattr` and no privilege
at all.

**This makes T88b redundant.** That task, in phase 9, is *"post-update port-access re-probe (`setcap`
is lost when the binary is replaced) and re-request if needed"* — which is this, done earlier and
unconditionally. T42 closes it rather than leaving two places describing one behaviour.

### D8 — Linux reads and writes the xattr directly, with no `libcap` and no shell-out

`getcap` and `setcap` come from the `libcap` package, which is not guaranteed present. The read side
runs on every daemon start on every Linux machine, so it may not depend on a package being installed;
and once `getxattr` is being called, `setxattr` is the same quantity of `unsafe` for the write side.
So both directions are syscalls, and `security.capability` is encoded and decoded here — a
`vfs_cap_data` header and two 32-bit masks, specified precisely enough to write in about twenty
lines.

Two items in `mixengine-platform` carry `#[expect(unsafe_code, reason = …)]` with a `# Safety`
section, which is the same lifting T41 D7 did for `chown` and the only place in the workspace where
the workspace-wide deny may be lifted.

### D9 — On macOS the probe compares files, and says what it does not prove

`/dev/pf` belongs to root, so the daemon — which runs as the user — cannot ask pf whether it is
enabled or what it has loaded. What it *can* read is `/etc/pf.anchors/mixengine`, the marker block in
`/etc/pf.conf`, and the plist, all world-readable. The probe is a byte comparison against what the
grant would write.

That proves the configuration is in place. It does not prove pf is running right now; the plist is
what makes that true at every boot, and the plist's presence is what the probe checks. The honest
end-to-end check is a request to `127.0.0.1:80` reaching this home's front end, which needs a front
end that serves something — T43's, and `mix doctor`'s (T47).

### D10 — The marker-block engine moves out of `hosts.rs`

T41 put `parse`, `splice` and `render` — everything that edits between `# BEGIN MixEngine` and
`# END MixEngine` without touching a line outside it — inside `mixengine-platform`'s `hosts` module.
`/etc/pf.conf` is the second file needing exactly that, with a different comment prefix and an
insertion point that is not the end of the file.

So that machinery becomes a `markers` module and `hosts` becomes its first caller. This is the
targeted kind of restructuring: it is forced by the second user, not proposed on taste.

### D11 — A capability on a user-writable binary, and why it is bounded

Granting `cap_net_bind_service` to a file the user can rewrite gives that user an ability they did
not have. What bounds it is the kernel, and it was measured rather than assumed: **any write clears
the capability**, by `cp` and by `mv` alike. So what root approved is the exact bytes that were there
when it approved them, and substituted code arrives with no capability and has to ask again.

What remains is a compromised daemon pointing the grant at a binary of its own choosing. Nothing in
the helper can distinguish that binary from a real front end. The control is T64: the path is printed
before the user approves it, which is what that task was built for.

### D12 — Grant and revoke share one dedupe key, and revoke ships without a producer

They are not two operations on a queue. They are two values of one question — *what port access
should this machine have?* — so they take the same `dedupe_key`, `"port-access"`, and T41 D2's
guarded upsert does the rest: a revoke enqueued behind a pending grant replaces it rather than
queueing after it, and the ordering problem never exists to be reasoned about.

**Nothing in T42 enqueues a revoke.** The producer of D7 asks in one direction only, and deliberately:
on Linux the probe needs the front-end binary's path, which is exactly what a home with no front end
cannot supply, so "no row, therefore withdraw the grant" is a question this OS cannot be asked. The
producer that can is uninstall (T87), which knows what it is removing.

So the operation ships built, validated and tested, with its only callers being the system tests and,
later, T87. That is the same shape T20, T21 and T22 landed in, and it is preferable to a reversal
invented at uninstall time against a grant written five phases earlier.

## The interface

```rust
// crates/mixengine-platform/src/traits/port_access.rs

/// How this machine lets a program the user runs answer on a port the OS reserves.
pub enum PortAccessMethod {
    /// Nothing is needed, and nothing is granted.
    Direct,
    /// A capability on the binary, which then binds the reserved port itself.
    Capability,
    /// A packet-filter redirect; the program binds an ordinary port instead.
    Redirect,
}

/// One port a site is reached on, and the port a program must bind to answer it.
pub struct PortBinding { pub answer: u16, pub bind: u16 }

pub struct PortAccessState {
    pub method: PortAccessMethod,
    pub bindings: Vec<PortBinding>,
    pub granted: bool,
    /// Why not, in words, when `granted` is false.
    pub missing: Option<String>,
}

pub trait PortAccess: std::fmt::Debug + Send + Sync {
    /// What this machine needs before `answering` can be served, and whether it is already there.
    ///
    /// Reads only. `binary` is consulted where the method is [`PortAccessMethod::Capability`] and
    /// ignored elsewhere.
    fn probe(&self, binary: &Path, answering: &[u16]) -> Result<PortAccessState>;
}
```

`Host` gains `fn port_access(&self) -> &dyn PortAccess;`, `mock::Host` gains a `PortAccess` that
answers from memory with `with_port_access` / `unable_to_probe_port_access` constructors in the shape
`with_hosts` / `unable_to_read_the_hosts_file` already set.

`mixengine-proto` gains `PortAccessPlan`, `PortRedirect`, the two `PrivilegedOp` variants, their
entries in `PrivilegedOp::ALL`, their `requires_elevation()` (both true), their `dedupe_key()` — one
key, `"port-access"`, shared by **both** variants, see D12 — and a `describe_*` for each, which is
what `mix elevation grant` prints.

## Crate changes

| Crate | Change |
| --- | --- |
| `mixengine-proto` | `PortAccessPlan`, `PortRedirect`, two `PrivilegedOp` variants, their descriptions |
| `mixengine-platform` | `traits/port_access.rs`; `markers` extracted from `hosts`; `port_access` per OS; the `security.capability` codec; the pf anchor, pf.conf block and plist writers behind `elevated`; `mock` |
| `mixengine-elevate` | `port_access.rs`: validation per D5, then apply; two arms in `ops.rs` |
| `mixengine-core` | nothing. The recipes are T43's |
| `mixengine-daemon` | the start-time producer of D7 |
| `.claude` | ADR 0012; the trait table and the `PrivilegedOp` list in `architecture/platform-abstraction.md`; the port sentence in `features/services.md`; T42 ticked and T88b closed by it in the roadmap |

## Testing

**Unit.** Anchor rendering; splicing and unsplicing the pf.conf block, including the insertion point;
`vfs_cap_data` encode and decode, including a capability set that is present but not the one wanted;
every refusal in D5; the probe's byte comparison.

**Platform, against the real OS, `TempDir` only, no `#[ignore]`.** Reading a capability back off a
file the test wrote; rendering a pf.conf block into a copy of the real `/etc/pf.conf`; the Windows
`Direct` answer.

**System, `#[ignore]`d, in the `system` job T41 built.**

| OS | What it proves |
| --- | --- |
| Linux | a file in a temp directory is granted the capability, the probe reads it back, `cp` over the file empties it, the probe reports the loss, and revoke clears it |
| macOS | granted, a server on 8080 is reached through `http://127.0.0.1/`, revoked, and it is not; and `/etc/pf.conf` is byte-identical to the file the test started with |
| Windows | the probe answers `Direct` and `granted`; both plans are refused `Unsupported` |

**The Linux row does not bind anything, and that is deliberate.** The system suite runs under `sudo`,
so a bind from inside it would succeed with no capability at all and prove the opposite of what it
claimed; proving it honestly means dropping privileges onto a second binary, for a fact
[run 32620072917](https://github.com/haiquang9994/mixengine/actions/runs/32620072917) already
measured end to end. What the suite is left to prove is the half that measurement cannot repeat on
every commit: that this code writes the attribute the kernel recognises, and notices when it goes.

The macOS line's second half is T41's own criterion — splice in, replace, take out, unrelated lines
untouched — applied to the second file this product edits.

**Daemon.** A home with a front-end row and a probe reporting no grant leaves one operation in the
queue and emits one `ElevationRequired`; a second start adds no second row.

## Out of scope, and where each goes

| Not here | There |
| --- | --- |
| A front end that actually listens on the granted port | T43 |
| A site reachable at `http://blog.test` end to end | T43 |
| Reporting port access in `mix doctor` and repairing it | T47 |
| A producer for `PortAccessRevoke` | T87, uninstall — D12 |
| Choosing the redirect target through the port allocator | nowhere yet; needs no wire change |
| An `inet6` redirect | nowhere yet; D6 |
| Whether an unsigned binary may do any of this under Smart App Control | T41a, now owed against the first release |

## Known limitation

**Two accounts on one machine share one grant**, and on macOS they share one anchor with one pair of
redirect targets. The second home's front end will want 8080 too and will fail to bind it. This is
the same debt T41 recorded for the hosts file, arriving for the same reason: the artifact is
machine-wide and the state it is generated from is per-home. It is recorded rather than solved.
