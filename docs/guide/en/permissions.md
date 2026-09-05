+++
title = "What MixEngine asks permission for"
slug = "permissions"
order = 11
summary = "Every administrator prompt MixEngine can raise, what each one literally changes, and why nothing of MixEngine's ever stays running as root."
+++

# What MixEngine asks permission for

A local development environment has to touch a few things that belong to the whole machine: the name
`blog.test` has to resolve, a browser has to trust a certificate, something has to listen on port
80. MixEngine's rule about all of it is short.

**Nothing MixEngine runs stays on your machine as an administrator.** Not the daemon, not the web
server, not a database. When a privileged change is genuinely needed, a small separate program
called `mixengine-elevate` is started through your operating system's own prompt, makes exactly that
change, and exits. It never runs a command somebody hands it; it knows the handful of operations it
is allowed to perform and checks each request itself rather than trusting the daemon that sent it.

## The prompts, and what each one changes

There are six, and you will normally meet them once.

### Routing the names

So that `blog.test` and everything under it reaches your own machine. MixEngine runs a small DNS
server that answers `127.0.0.1` for every name under a managed suffix, and what needs permission is
pointing your system at it — a file under `/etc/resolver/` on macOS, a resolver rule on Linux, an
NRPT rule on Windows.

This is asked **once**, not once per site, and that is the whole reason the DNS server exists: an
approach that edited the hosts file would need your password again every time you created a site.
Where the resolver route is not available, MixEngine falls back to one exact line per name in the
hosts file, inside a clearly marked block it owns and can remove again.

### Trusting the certificate authority

MixEngine issues its own certificates so that your sites are `https://` with no warning. For a
browser to accept them, the authority that signed them has to be in your system's trust store, and
putting it there needs permission.

**What this does and does not mean.** The authority is generated on your machine and its private key
never leaves it. It can vouch for any name, so it is worth understanding that installing it is a
real trust decision — the same one every local-HTTPS tool asks for. Declining is a supported answer:
your sites keep working over `http://`, and MixEngine says so rather than failing.

On Linux, Chrome and Firefox read their own certificate databases rather than the system store, so
MixEngine writes there too — which needs no administrator at all, because those files are yours.

### Listening on port 80 and 443

On macOS and Linux, ports below 1024 are privileged. MixEngine does not solve this by running the
web server as root — it grants the ability to the one program that needs it and nothing else, and
then the server runs as you.

### A firewall rule, when you share a site

Only when you ask a site to be reachable from your phone or another machine on the same network. The
rule is for that one port, it is removed when you stop sharing, and nothing else about your firewall
is touched.

### Installing the privileged helper

`mixengine-elevate` itself has to live somewhere you cannot write to — a program that runs as an
administrator and sits in a directory any process could overwrite is not a security boundary. So the
first privileged thing MixEngine ever does is put the helper in place. Four of the ways of
installing MixEngine run entirely as you (the Windows installer, the portable zip, the AppImage, and
building from source), which is why this cannot be the installer's job. Where a `.deb`, an `.rpm` or
a `.pkg` has already placed it, MixEngine notices and asks for nothing.

### Replacing the privileged helper

Updates never touch it. `mix self-update` replaces the daemon and the client and deliberately leaves
`mixengine-elevate` exactly as it was; `mix elevation upgrade` is the separate, deliberate act that
fetches a new one, and the helper already installed checks MixEngine's signature on the replacement
before it allows itself to be overwritten.

## One prompt, not six

MixEngine collects what needs permission and asks once. On a fresh machine, creating your first
HTTPS site typically means a single prompt covering the resolver rule, the certificate authority and
the port grant together.

You can see the queue before anything is asked:

```bash
mix elevation status
```

That prints every operation waiting and what it will change — the exact hosts lines, the port, the
store. Then, when you are ready:

```bash
mix elevation grant
```

**Saying no is a normal answer.** The queue stays where it is, nothing is half-applied, and you can
run the command again later. If you decide an operation should never be asked about again:

```bash
mix elevation drop <id>
```

## The audit trail

The helper writes a line for every privileged operation it performs, in a file only an administrator
can change:

| System | Path |
| --- | --- |
| Windows | `%ProgramData%\MixEngine\elevate.log` |
| macOS | `/Library/Logs/MixEngine/elevate.log` |
| Linux | `/var/log/mixengine/elevate.log` |

That file and the helper itself are the only two things MixEngine leaves outside its own directory.
`mix doctor` reports both and removes neither — a diagnostic that deleted a root-owned audit trail
would be deleting the record of what it was diagnosing. `mix uninstall` is what takes them away, and
it asks.
