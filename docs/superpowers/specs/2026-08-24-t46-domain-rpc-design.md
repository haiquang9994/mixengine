# T46 — `domain.*` and the diagnostic that answers "why doesn't it work?"

Roadmap task **T46**, phase 4. Design settled 2026-08-24, before implementation.

T45 gave a machine a way to send a managed name to this daemon's DNS server. This task gives a
person a way to ask **what actually happens to one name**, and gives a client the two domain verbs
it cannot compose for itself.

## What this is for

`http://blog.test` failing has four independent causes, and every one of them looks identical from a
browser: the name is not declared, no hosts line was written, no resolver routes the TLD, or the
server is not answering. A user can currently distinguish them only by reading `daemon.status`,
knowing what `hosts_only` implies, and finding the hosts file themselves.

The failure this task is designed against is subtler, and was measured during T45: **a diagnostic
built with the wrong instrument reports a working machine as broken.** `nslookup` on Windows talks to
the configured server directly and does not honour the Name Resolution Policy Table — it answers
NXDOMAIN for a name `getaddrinfo` resolves at the same moment. A tool that shipped with `nslookup`
inside it would send every correctly wired Windows user looking for a fault that is not there.

## Scope

In: `domain.add`, `domain.remove`, `domain.dns_status`, their CLI commands, and the errors they need.

Out, and each has an owner: reconciling or repairing anything is **T47** (`mix doctor`,
`doctor_repair`); certificates are **phase 5**, so the feature spec's "aliases share the site's
certificate SANs, which triggers a cert reissue" describes machinery that does not exist yet and is
not built here; a diagnostics archive is **T93**.

---

## D1 — Three methods, and why two of them exist at all

```
domain.add    { site, domain, accept_risky_tld? }  ->  SiteDetail
domain.remove { domain }                           ->  SiteDetail
domain.dns_status { domain? }                      ->  DomainStatusReport
```

`site` is a `SiteRef`, the identifier every `site.*` method already takes, and the answer to both
verbs is the `SiteDetail` the site now is — so a client that adds a domain renders the result without
a second call.

`site.update` already carries `domains` and replaces the list wholesale, so `add` and `remove` add no
capability. They exist because of what a client would otherwise have to do: read the site, append one
name, send the whole list back. That is business logic in a client, which `CLAUDE.md` forbids
outright, and it is a read-modify-write that silently drops a domain another client added in between.
One method meaning "add this one" has neither problem.

They are **thin over the existing path**: each resolves the site, computes the new list, and calls the
same `sites::update` that `site.update` calls — the same TLD check, the same hosts queueing, the same
front-end re-render. No rule gets a second copy.

## D2 — `remove` takes a domain and no site

`0001_initial.sql` has `CREATE UNIQUE INDEX site_domains_domain`, commented there as "the one that
decides ownership": a domain belongs to exactly one site in a home. Asking the caller for the site as
well would be asking for a fact the database already holds, and would let a caller name a site the
domain is not on.

## D3 — Two refusals, both by name

**Removing a site's last domain is refused.** The schema records "at least one" as an invariant the
site module upholds because SQLite cannot express it, and a method that could empty the list would be
the one place that breaks it.

**Removing a site's primary domain is refused.** The primary decides the site's canonical URL and,
from phase 5, the name on its certificate. Silently promoting another domain would change *what the
site is* under a method called "remove a domain" — a larger act than the one requested. `site.update`
reorders the list and the first entry is primary, which is where changing a primary belongs.

Neither refusal is a fallback for the other: a site with one domain hits the first, a site with
several hits the second, and each message names which.

## D4 — `dns_status` reports four facts and no verdict

```rust
pub struct DomainStatus {
    /// The name asked about, normalised.
    domain: String,

    /// The site that declares it, or `None` for a name nothing does.
    site: Option<String>,

    /// Is there a line for this name in the managed hosts block, on disk, now.
    hosts_entry: bool,

    /// Does a wired TLD cover it — so subdomains work without anything being written down.
    wildcard: bool,

    /// What this daemon's own DNS server answers, asked over its socket.
    server_answers: Option<Ipv4Addr>,

    /// What the operating system actually resolves it to.
    resolves_to: Vec<IpAddr>,

    /// One sentence when this name will not work, or `None`.
    because: Option<String>,
}

pub struct DomainStatusReport {
    /// One row per name asked about, in domain order.
    domains: Vec<DomainStatus>,
}
```

A report wrapping the list rather than a bare `Vec`, on `SiteList`'s precedent: the one-domain and
every-domain questions then have one answer shape, and a later field — a timestamp, a note about a
lookup that timed out — has somewhere to go that is not inside every row.

Four separate facts rather than one verdict, because **they fail independently**: a hosts line with no
server, a server with no resolver, a resolver wired to a TLD this name is not on. Collapsing them into
a boolean is precisely what `DnsStatus::wildcards` had to stop doing in T45, for the same reason — the
caller is then left to work out which half of the answer applies to it.

`because` is a sentence rather than a code, on `DnsStatus::because`'s precedent next door. It says what
is wrong. It does not say what to do about it: repair is T47's, and a diagnostic that suggests a fix it
cannot perform is a diagnostic that will drift from the thing that performs it.

## D5 — A name nothing declares is reported, not refused

`site` is an `Option`. Someone asking why `foo.test` does not work when they never declared it should
be told exactly that, and the other three facts still hold answers — the TLD may be wired, the server
answers for any name under a wired TLD, and the OS may well resolve it. Refusing the question would
withhold the answer to it.

The domain is still normalised and syntax-checked through `core::domains`, so a request carrying
something that is not a domain at all is refused as one.

## D6 — The real lookup is `getaddrinfo`, and never `nslookup`

`std::net::ToSocketAddrs`, on `spawn_blocking`. **Measured in T45**: `nslookup` bypasses the NRPT on
Windows and would report a correctly wired machine as broken. The instrument has to be the one the
operating system gives ordinary programs, because that is the population the answer is about.

**The OS cache is included, deliberately.** T45's system test had to defeat the cache — a fresh name
each poll — because it was asking whether a *mechanism* works. This asks a different question: what
does the user's browser see right now, and the cached answer is that answer.

`spawn_blocking` cannot be cancelled. A bounded `tokio::time::timeout` therefore stops the daemon
*waiting*; it does not stop the lookup. The worst case is one blocked thread per domain asked about,
released whenever the resolver gives up. Written down rather than papered over, because a timeout that
reads like a cancellation is how a thread leak becomes invisible.

## D7 — The server is asked over its socket

A real UDP query to the address the server is listening on, not a call into the zone it answers from.
Asking the zone proves the answering logic; asking the socket proves **the listener**, which is the
only fact that separates "the server died" from "nothing routes a name to it" — the two failures this
report exists to tell apart.

## D8 — The CLI

```
mix domain add <domain> --site <site> [--i-know]
mix domain remove <domain>
mix domain status [<domain>]
```

`--i-know` is the existing spelling of `accept_risky_tld` for `.local`. `status` with no domain is
every declared domain in the home, as a table; with one, that one. `--json` throughout, as everywhere
else.

## D9 — Testing: prove the instrument, then the machine

The valuable half is that the report is **honest when the machine is broken**, which is the state CI
runs in: the ordinary `test` job wires no resolver, so a name under `.test` must be reported as not
resolving — and that is the assertion.

Which makes a control mandatory, on T45's D14: `localhost` must resolve through the *same* instrument
in the *same* run. Without it, "blog.test does not resolve" is a statement about `getaddrinfo` rather
than about the machine, and four of the six measurement rounds behind T45 were void for exactly that
reason.

No elevation anywhere: all four facts are reads. The hosts file is world-readable, the resolver probe
was built unprivileged in T45 precisely so the daemon could afford it on every start, and the server
is on loopback.

## Errors this adds

| Variant | Code | When |
| --- | --- | --- |
| `LastDomain { domain }` | `Conflict` | it is the site's only domain |
| `PrimaryDomain { domain }` | `Conflict` | it is the site's primary |

Two and not three. `domain.remove` on a name nothing declares needs no error of its own: it resolves
the site through `Sites::expect`, whose `SiteRef::Domain` arm already answers `NotFound` with "no
site answers to {domain}" and the hint that `mix site list` shows what does. A second sentence for
one condition is a second sentence to keep in step with the first.

`DomainTaken`, `InvalidDomain`, `UnmanagedTld` and `RiskyTld` already exist and already say what
`domain.add` needs.

## What this task does not settle

Whether `mix doctor` renders this report or computes its own. **T47 decides, and should render this
one** — a second implementation of the four facts is a second answer to one question.
