# T43 — Site → config → reload, end to end

**Roadmap:** T43, `.claude/roadmap/phase-4-sites-and-elevation.md`
**Depends on:** T30 (the generation engine, staging and atomic install), T31 (the Caddy recipe and
`import sites/*.caddy`), T37 (nginx, `Role::FrontEnd`, `core::services::front_end`), T32 (php-fpm
pools), T39/T39a (projects, the three site tables, the four kinds), T41 (the hosts entries a
declared name needs), T42 (`PortAccess` and `PortBinding`)

## What this closes

Every piece of `http://blog.test` exists except the middle. T39a writes a site down; T31 and T37
render a front end whose configuration imports a directory of site files; T41 queues the hosts entry
that makes the name resolve; T42 arranges for the front end to be *allowed* to answer on 80. Nothing
writes a single site file, so the glob in both front-end templates matches nothing and every site in
the database is a row nobody serves.

T43 is that middle, and the two methods that drive it: `site.start` and `site.stop`.

## What already exists, and is reused unchanged

- **The staging install** (T30): `document::install` compares each rendering against what is on
  disk, stages the whole set in a sibling directory, lets the service's own checker judge it *there*,
  and only then commits file by file. A set that is entirely unchanged is not even validated. This is
  what makes "idempotent re-runs" a property rather than a feature — see D10.
- **`import sites/*.caddy` and `include sites/*.conf`** (T31, T37), both resolving against the
  directory holding the file they are written in, which is the staging directory while the checker is
  looking and `etc/<service-id>/` afterwards. T31 put them there rather than in Phase 4 precisely so
  that whoever rendered the first site would render it into the set being judged.
- **The reload** (T31): `Generated::changed()` → `Registry::hand_over` → a permit on the runner's
  `Notify` → `caddy reload` / `nginx -s reload` in the service's own `Surroundings`. T43 changes a
  file and inherits the reload; it does not add a mechanism.
- **`Role::FrontEnd`** (T37) and `core::services::front_end::held_by`, which answer "which service is
  a site reached through" by what a package is *for* rather than by its name.
- **The hosts queue** (T41): `Sites::wants_the_hosts_file` already runs on every write and is not
  touched here.
- **`PortAccessState::bindings`** (T42): one `PortBinding { answer, bind }` per port, the value that
  keeps `#[cfg]` out of `mixengine-core`.

## Decisions

### D1 — Site files belong to the front end's own document set

They are not installed by a second path that runs after the recipe. `Generator::render` sees
`recipe.role() == Role::FrontEnd` and appends the site documents to the `Vec<Document>` the recipe
already produces, so staging, validation, atomic install and the reload decision are the ones that
already exist.

The alternative — a `core::generate::sites` module that writes `etc/<front-end>/sites/` on its own —
fails for one specific reason, and it is the reason T31 gave when it put the glob where it did. The
checker judges a *staging directory*. A site file installed outside that set is absent while
`caddy validate` is looking and present when the server reads the configuration: the one arrangement
whose correctness cannot be checked before it is live.

### D2 — The hook is a recipe method, and the role is what selects it

```rust
fn sites(&self, context: &Context, served: &[Served]) -> Result<Vec<Document>>
```

Default body returns an empty `Vec`, so twelve recipes are unaffected and two implement it. A recipe
is already the thing that knows how to turn state into this program's file format; a `match` on the
package name in `Generator` would be that knowledge moved somewhere it does not belong, and it is
exactly what `Role` was introduced to avoid.

`Generator` passes the same `&[Served]` to whichever recipe answers `FrontEnd`. A home has at most
one — `service.create` refuses a second (T37) — so there is no question of which one gets them.

### D3 — One set, one judgement; `Degraded` is deferred and the promise is corrected

`.claude/features/services.md` says today that a site whose configuration is broken "just fails
validation and is skipped, with the site marked `Degraded`". T43 does not do that, and the line is
corrected rather than left to be discovered.

Two reasons. First, `SiteState` has two words on purpose (T39a): a site is not a process. A third
word costs a database `CHECK`, a wire variant, a CLI column and a rule about how a site leaves that
state, and it would be bought for a case that cannot arise yet. Second — and this is the honest half
— **a site carries no free text today.** Its domains are normalised by `core::domains`, its doc root
is refused if it resolves outside the project, its upstream is checked to be an absolute `http`/`https`
URL with a host, and its pool is a `ServiceId` that must exist. Every one of those is validated
before the row is written. A rendered site file that `caddy validate` refuses is therefore a bug in
this repository's template, not a mistake a user made, and skipping it would hide the bug while
serving eleven sites out of twelve.

So: the whole set is judged together, a refusal installs nothing, the front end goes on reading the
configuration that worked, and the error names the file the checker complained about. `Degraded`
becomes real when a site can carry a snippet somebody wrote — the extension surface
(`.claude/features/extensions.md`) is where that arrives, and the deferral is written into
`services.md` beside the corrected sentence.

### D4 — Orphan removal is a swept directory, declared by the recipe

T31 left this open in as many words: *"a deleted site whose import file survives is a site that goes
on being served"*. The mechanism is one new idea in `document::install`:

```rust
pub async fn install(
    directory: &Path,
    documents: &[Document],
    swept: &[&str],                  // relative directories whose contents must equal the set
    validator: Option<&Validator>,
) -> Result<Vec<Written>>
```

A swept directory's contents must be exactly the documents rendered into it. Anything else in it is
removed. Two properties matter:

- **The sweep happens in staging, before the checker runs**, so `caddy validate` judges the set that
  will exist rather than the set plus whatever is left over. A stale `blog.test.caddy` naming a
  deleted pool would otherwise pass validation on the way in and be read on the way out.
- **A file that has to be removed counts as a change**, so a home whose only difference is a deleted
  site still reloads. Without that, `mix site delete` would leave the old site being served until
  something else changed.

Only the front-end recipes declare a swept directory, and each declares exactly `sites/`. Nothing
sweeps `etc/<service-id>/` itself: a directory belonging to a *deleted service* is `service.delete`'s
problem and is not made T43's by proximity.

A `Disabled` site travels this path with no special case. It is not in `served`, so no document is
rendered for it, so the sweep removes the file it used to have — which is the whole meaning of
"declared and deliberately not rendered".

### D5 — A pool's address is computed once, by the recipe that owns it

`fastcgi_pass` needs to know where php-fpm listens: `run/php-fpm-<version>.sock` on Unix,
`127.0.0.1:<row port>` on Windows. Both are computed today inside `php_fpm`'s `spec()`. A site
template that worked either of them out again would be a second copy of a rule whose whole point is
that it differs per OS.

```rust
pub enum Upstream {
    Socket(PathBuf),
    Tcp(SocketAddr),
}

// on Recipe, default `Ok(None)`
fn upstream(&self, context: &Context) -> Result<Option<Upstream>>
```

`php_fpm` implements it and `php_fpm::spec()` is rewritten to call it, so the socket path in the
pool's own `php-fpm.conf`, the path in its readiness check and the path in every site's
`fastcgi_pass` are one expression evaluated once.

Rendering a site therefore needs the pool's `Context`, which is built from the pool's row.
`Generator` grows a two-pass shape: build a `(Recipe, Context)` for every declared row, collect the
`ServiceId → Upstream` map from that, then render. The passes are cheap — a `Context` is built from
a row already fetched, and nothing is written until the second.

**A php-fpm site whose pool is gone is not rendered, and does not fail the render.** T39a made that
state reachable on purpose: `pool` is an `Option`, and `service.delete --force` is allowed to cross a
site's declaration — it *"crosses the declaration and never the running process"*. If a missing pool
failed generation, that one `--force` would leave a daemon that cannot render anything at all, which
is a far worse outcome than the site it was about. So the site is left out of `served`, its file is
swept away, and a line goes in `daemon.log`. Serving it any other way would mean answering PHP with
something that is not PHP. Reporting it to a person is `mix doctor`'s (T47), which already exists to
reconcile stale declarations.

### D6 — nginx's `fastcgi_params` comes out of the package

`mixengine-packages`' nginx artifact already publishes it. `tools/nginx.py` lists
`fastcgi.conf, fastcgi_params, mime.types, scgi_params, uwsgi_params` under `CONF_FILES` with the
reason spelled out: *"the other four are what a `fastcgi_pass` to PHP-FPM needs, which is the reason
MixEngine renders an nginx config in the first place."*

So it joins `mime.types` in `Endpoints::includes` and the site template writes
`include "<absolute path>";`. Writing the seventeen `fastcgi_param` lines into the template by hand
would be this repository maintaining a copy of a file the package already ships and the server
already reads.

Caddy needs no equivalent: `php_fastcgi` sets the same variables itself.

### D7 — `node-app` renders as a reverse proxy to loopback, and that is all it is

The roadmap said this before the task started: *"nothing in this roadmap supervises a node process.
`node-app` is a declaration; if T43 renders it identically to `reverse-proxy`, that is the honest
outcome and belongs written down there."* It does, and it is written down here.

The difference between the two kinds survives only in the address: `node-app` names a port and the
host is forced to `127.0.0.1`, while `reverse-proxy` names a whole URL that may point anywhere.
Nothing starts `npm run dev`, and nothing in this build pretends to.

### D8 — The row keeps the answer port; the template renders the bind port

`services.port` for a front end is what a browser asks for. What the process must listen on is
`PortBinding::bind`, which on macOS is 8080 for 80 and 8443 for 443 and on the other two systems is
the same number. Rendering that is T43's, and `.claude/features/services.md` already says so.

So the front-end templates render the *mapped* value: `http_port` is the binding for the row's port,
`https_port` is the binding for the `https_port` setting. The row is not rewritten — a row holding
8080 would make the answer port unrecoverable and would break LAN sharing (T74), which is about
`bind_addr` and the port a site is *reached* on.

Getting the mapping to `mixengine-core` needs one addition to the platform trait:

```rust
fn bindings(&self, answering: &[u16]) -> Vec<PortBinding>;
```

**Pure: no file is read and no binary is named.** All three implementations already build this vector
inside `probe`, and this is that expression given a name. It has to be free of I/O because a
`Generator` is constructed once for the life of the daemon and the mapping is a constant of the
operating system — reading an xattr or `/etc/pf.conf` to learn that macOS redirects 80 to 8080 would
be an answer that cannot change bought at the price of a syscall. `probe` calls it, so there is one
table.

`Generator::new` takes the resulting `Vec<PortBinding>`; `services::spec::declared` computes it from
`host.port_access().bindings(&[80, 443])`.

A site's own block names no port. Both servers take the listening port from their global section, so
a Caddyfile site address stays `http://blog.test` on all three systems and only the global block
differs. The `Host` header a browser sends still says `blog.test`, which is what the site is matched
on, so a redirect on macOS is invisible above the packet filter.

### D9 — `site.start` and `site.stop` set a flag and walk; they never touch a process

`site.start` is `state = enabled`, render, reload. `site.stop` is `state = disabled`, render, reload.
Neither starts, stops or asks after a service, and a home whose Caddy is not running gets a rendered
site file and no traffic.

This is T39a's line held rather than a limitation: *"a site is not a process; `starting`, `running`
and `failed` belong to the services it uses, which have seven states of their own."* A `site.start`
that started the front end would be a `site.*` method owning a process, and it would then owe an
answer to what `site.start` means when the pool starts and the front end does not.

Both take `SiteQuery` and answer `SiteDetail` — the same request and the same answer as `site.show`,
because what a caller wants back is the site as it now is. `SiteState` already travels on
`site.update`, so these two are reachable-by-other-means today; they exist because "start this site"
is the sentence a person says, and `.claude/features/client-surface.md`'s rule is that a client
renders what the daemon returns rather than composing an update to express a verb.

### D10 — A write renders synchronously, and a refusal fails the call

Every mutating `site.*` call — `create`, `update`, `delete`, `start`, `stop` — ends by asking the
registry for a walk, which renders, validates, installs and hands over the reload. `Sites` therefore
gains `Arc<Registry>`.

If the walk fails the call fails, and nothing was installed: `document::install` stages first, so the
front end is still reading the configuration that worked. This is deliberately *not* the hosts
queue's behaviour, which never fails an operation, and the two differ because of what a failure
means. A hosts entry that has not been granted yet is a want with a person on the other end of it;
`mix status` goes on saying so. A configuration the server refused is a defect, and a `site.create`
that returned success while the site was unreachable would send the person who typed it to look in
the wrong place.

A home with no front end renders nothing and the call succeeds — there is no set to add sites to,
which is a different thing from a set that was refused.

**Idempotence falls out and is not implemented.** A second `site start` on an enabled site rewrites
the same bytes; `document::install` compares before it stages, finds nothing changed, skips the
validator entirely and reloads nothing.

### D11 — A site address is written `http://`, and `https_enabled` stays a declaration

Caddy turns a bare `blog.test` into an HTTPS site and tries to obtain a certificate for it. The
recipe already sets `auto_https off` for that reason, and the site block spells the scheme anyway, so
the two say the same thing and a Phase 5 change to the setting cannot silently alter what a site
means.

`https_enabled` on the row is read by nobody in T43. Phase 5 owns it, and rendering half of it now
would leave a site that redirects to a port serving nothing.

### D12 — A site file is named after its primary domain

`sites/blog.test.caddy`, `sites/blog.test.conf`. Domains have been through `core::domains::normalised`
by the time a row exists — trimmed, lowercased, ASCII, one of the managed TLDs — so there is no
character in one that needs escaping in a filename on any of the three systems.

The row's integer id was the alternative and is rejected for what it costs a person: `etc/caddy/sites/7.caddy`
tells whoever is reading the directory nothing, and the directory is one of the first places somebody
looks when a site does not answer. A domain that moves to another site renames the file, and the
sweep in D4 is what makes that safe.

## The interface

```rust
// mixengine-platform, on the PortAccess trait
fn bindings(&self, answering: &[u16]) -> Vec<PortBinding>;

// mixengine-core::generate::served
pub struct Served {
    /// Ordered; the head is the primary.
    pub domains: Vec<String>,
    /// Absolute: the project's root joined to the row's relative doc root.
    pub doc_root: PathBuf,
    pub kind: ServedKind,
    /// Read by Phase 5. Rendered by nothing here.
    pub https: bool,
}

pub enum ServedKind {
    PhpFpm { upstream: Upstream },
    Static,
    ReverseProxy { upstream: String },
    NodeApp { port: u16 },
}

pub enum Upstream {
    Socket(PathBuf),
    Tcp(SocketAddr),
}

/// Every enabled site, joined to the project that gives its doc root a root, with each php-fpm
/// site's pool resolved through `upstreams`. A site whose pool is not in the map is left out.
pub(super) async fn served(
    store: &Store,
    upstreams: &BTreeMap<ServiceId, Upstream>,
) -> Result<Vec<Served>>;

// mixengine-core::generate::recipe, on Recipe — all three default to nothing
fn sites(&self, context: &Context, served: &[Served]) -> Result<Vec<Document>>;
fn swept(&self) -> &'static [&'static str];
fn upstream(&self, context: &Context) -> Result<Option<Upstream>>;

// mixengine-core::generate
impl Generator {
    pub fn new(
        paths: Paths,
        store: Store,
        catalogue: Catalogue,
        bindings: Vec<PortBinding>,
    ) -> Self;
}

// mixengine-proto::rpc::method
pub const SITE_START: &str = "site.start";
pub const SITE_STOP: &str = "site.stop";
```

`site.start` and `site.stop` take `SiteQuery` and answer `SiteDetail`; no new request or response
type is added.

## Crate changes

**`mixengine-platform`** — `PortAccess::bindings`, implemented on all three systems by lifting the
expression already inside each `probe`, and on the mock. No new dependency, and nothing in the
elevated closure moves.

**`mixengine-proto`** — the two method names. `SiteQuery` and `SiteDetail` are unchanged.

**`mixengine-core`** — `generate::served` (the new module: `Served`, `ServedKind`, `Upstream`, and
the query that assembles them from `sites`, `site_domains` and `projects`); three defaulted methods
on `Recipe`; the swept-directory parameter on `document::install`; `Generator`'s two-pass render and
its `bindings`; `php_fpm::upstream` with `spec()` rewritten onto it; `Caddy::sites` +
`caddy/site.caddy`; `Nginx::sites` + `nginx/site.conf` + `fastcgi_params` in its `endpoints`; the
`http_port`/`https_port` mapping in both front-end templates.

**`mixengine-daemon`** — `Sites` gains `Arc<Registry>` and a walk at the end of every write;
`sites::start` and `sites::stop`; two arms in `api/rpc.rs`; `services::spec::declared` computes the
bindings and passes them to `Generator::new`.

**`mixengine-cli`** — `mix site start <site>` and `mix site stop <site>`, rendered through the
existing site detail renderer.

## Testing

**Unit, in `core::generate::served`** — a doc root comes back absolute and normalised; a `Disabled`
site is absent from the set; a php-fpm site's upstream is the pool's socket on Unix and its port on
Windows; a site whose pool row has been deleted is reported rather than rendered with an empty
address.

**Parity, both front ends, in `core::generate`** — T37's precedent, one suite driving Caddy and
nginx over the same input: each of the four kinds renders a block naming the doc root or the upstream
it was given; a site with three domains is reachable at all three; a `node-app` renders the same
proxy a `reverse-proxy` to `127.0.0.1:<port>` renders, which is D7 asserted rather than described.

**The sweep** — a file left in `sites/` that no site owns is gone after a render, the render counts
as changed because of it, and a file in `etc/<service-id>/` outside a swept directory is left alone.

**The port mapping** — a rendered Caddyfile and `nginx.conf` carry 8080/8443 when the bindings say
`Redirect` and 80/443 when they say `Direct`, with the row unchanged in both.

**Daemon** — `site.start` twice writes nothing the second time and reloads nothing; `site.stop`
removes the file; a validator that refuses fails the call and leaves the installed configuration
alone; a home with no front end accepts a `site.create` without error.

**End to end, against the real server** — in `crates/mixengine-cli/tests/harness/frontend.rs`, the
arc both `caddy.rs` and `nginx.rs` already run, so this is one sequence of assertions judged by two
servers and **no CI change**: the `test` job fetches both archives today and runs each suite in its
own step. `#[ignore]`d, as that harness already is. Install the front end, create a project and a
static site, start the service, then request `127.0.0.1:<http_port>` with `Host: blog.test` — the
harness' own `request` helper, whose documentation already says the header has to be the site's own
address — and assert the file's contents come back. Then `mix site stop`, and assert the same request
is answered by a server that no longer knows the name.

The `Host` header rather than the name: CI has no elevation, so no hosts entry exists, and what this
suite is for is proving that the rendering is right and the server is reading it — not that a name
resolves. Resolution is T44/T45's and has its own suites.

## Out of scope, and where each goes

- **DNS.** `blog.test` resolves through the hosts entry T41 queues. The wildcard resolver is T44 and
  T45.
- **TLS.** `https_enabled` is stored and unread. Phase 5.
- **`Degraded`.** D3, and the extension surface is where a site first carries text somebody wrote.
- **Supervising a node process.** D7.
- **Per-site access logs.** `access_log off` stays; the nginx template already names T43 as the task
  that would decide otherwise, and the decision is *not yet*: MixEngine captures and rotates what a
  service writes to its stream, and a second set of files growing under `logs/` that nothing rotates
  is what that capture exists to prevent. A per-site log becomes worth having when there is something
  that reads it.
- **Switching front ends.** T37 permits one front-end row, so a switch is a delete and a create; the
  next walk renders the new one's `sites/` in full. The old `etc/caddy/` left behind belongs to
  `service.delete`.

## Known limitation

A site whose project directory has been moved or deleted still renders. The doc root is a path in the
database, and neither front end refuses a configuration naming a directory that is not there —
Caddy's `file_server` answers 404 and nginx answers 403. Checking it at render time would mean a
`stat` per site on every walk and a rendering that fails because of something outside `MIXENGINE_HOME`;
reporting it belongs to `mix doctor` (T47), which already reconciles stale state and has the place to
say it.
