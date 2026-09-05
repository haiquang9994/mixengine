+++
title = "Names and the padlock"
slug = "domains-and-https"
order = 7
summary = "How blog.test reaches your machine, which certificate signed it, and how to find out when the padlock is not green."
+++

# Names and the padlock

Two things have to be true before `https://blog.test` opens without a warning. The name has to reach
your own machine, and your browser has to accept the certificate it is offered. MixEngine arranges
both, and this page is about what it actually did.

## Which names you may use

| Suffix | Status |
| --- | --- |
| `.test` | **The default.** Reserved by the standards body for exactly this, never resolvable on the internet, and it cannot collide with anything real |
| `.internal` | Also managed. Reserved as the private-use suffix, and it reads as an intention where `.test` reads as an experiment |
| `.localhost` | Offered as the zero-configuration option: many systems already send `*.localhost` to loopback, so it needs no change at all |
| `.local` | Supported, and warned about — see below |
| `.dev`, `.app`, … | **Refused.** They are real, public, and browser-pinned to HTTPS; taking one over locally breaks the real internet for you |

**`.local` belongs to mDNS**, which is how printers and speakers announce themselves on a network.
Using it works until somebody plugs one in. MixEngine will let you, but the CLI makes you say
`--i-know`, and it never points a *resolver* at `.local` — a site there gets one exact hosts entry
and nothing more, because sending every `.local` name to loopback would break every Bonjour device
on your network.

## How the name reaches you

MixEngine runs a small DNS server of its own that answers `127.0.0.1` for **every** name under a
managed suffix, at any depth, whether or not a site has been declared for it. That is what makes
`api.blog.test` and `staging.blog.test` work without anyone declaring them.

Pointing your system at that server needs permission **once**. A hosts file, in contrast, would need
your password every time you created a site, which is the whole reason the DNS server is the primary
mechanism and the hosts file is the fallback. Where the resolver route is not available, MixEngine
writes one exact line per name, inside a marked block that it owns and can remove again.

`AAAA` queries are answered with no records rather than with `::1`, deliberately: the front end
listens on IPv4, and a name resolving to an address nothing is listening on is a browser that waits
before falling back.

## Adding and removing names

```bash
mix domain add api.blog.test --site blog.test
mix domain remove api.blog.test
```

A name added this way is an **alias**. The site's primary domain does not change, because the
primary is what the canonical URL and the certificate are named after. Removing is refused for a
site's last domain and for its primary — `mix site update` is what reorders them, and the first
`--domain` it is given becomes the primary.

## When a name does not work

```bash
mix domain status blog.test
```

This is the diagnostic to reach for, and it is built to fail one part at a time rather than saying
"broken". It answers four separate facts: whether the name is declared, how it is routed, whether it
actually resolves on this machine right now, and whether something answers on it. With no argument
it does that for every name this MixEngine knows.

## The certificate authority

MixEngine issues its own certificates rather than using a public authority, because local names are
not publicly resolvable and no public authority will sign them. So there is an authority on your
machine, generated on first use, whose private key never leaves it.

```bash
mix cert ca-status
```

That says what the authority is — its name, its fingerprint, how long it has. Whether your machine
*trusts* it is a separate question about your operating system's stores, and this build does not
answer it here; nothing printed by `ca-status` implies an answer to it.

On Linux there are two trust answers rather than one, and MixEngine keeps them apart: the system
store, and the separate certificate databases Chrome and Firefox read instead. A tool that collapsed
them would show a green tick beside a browser showing a red padlock.

## Certificates for sites

Leaf certificates are per site, 90 days, covering exactly that site's domains in that site's own
order.

```bash
mix cert issue --site blog.test
mix cert issue            # every HTTPS site
```

Issuing is **idempotent**: a certificate that still covers the right names, has more than thirty
days left and was signed by the authority you have now is left exactly as it is. So running it costs
nothing and is a reasonable thing to do when you are unsure.

## Is the padlock actually green?

```bash
mix cert status
```

This does not read the disk. It opens a real TLS connection to your own front end for every site and
reports the certificate that was actually presented — which is the only thing a browser ever sees,
and the only way to notice a server still holding a certificate that was replaced underneath it. It
reads only: nothing is issued, nothing is installed, nothing is reloaded.

## Replacing the authority

```bash
mix cert ca-rotate
```

**Destructive.** Every browser holding a cached chain under the old authority stops accepting it,
and every site's certificate is reissued. Nothing is replaced unless this machine can be made to
trust the new authority — declining the prompt leaves everything exactly as it was.

To stop trusting MixEngine's authority without removing anything else:

```bash
mix cert ca-uninstall
```

That takes the authority out of every store that trusts it and leaves both the certificate on disk
and every site's certificate alone. `mix doctor --repair` puts the trust back.
