+++
title = "Keeping MixEngine current"
slug = "updating"
order = 12
summary = "Updates are opt-in, checked against a signature, and rehearsed before anything is replaced — and one binary is deliberately never replaced this way."
+++

# Keeping MixEngine current

```bash
mix self-update --check
mix self-update
```

`--check` prints what is available — the version, the size, and what changed — and installs nothing.
Without it, the same information is shown and then you are asked.

## Updates are never silent

An update restarts the services you are running. That makes it a thing you choose, not a thing that
happens to you while you are working, so **nothing is ever installed without being asked**. The
daemon does check quietly — at start, and on a daily clock — so that `mix status` can tell you an
update exists, and both checks fail silently: a machine with no network is not a machine with a
problem.

`--yes` answers the question in advance, for a script with nobody at the keyboard.

## What happens when you say yes

In order, and none of the steps is optional:

1. The release is downloaded and hashed against the **signed** update feed. A payload that does not
   match is not unpacked.
2. The signature is checked against a public key compiled into MixEngine itself. Nothing about the
   transport is trusted to decide whether a file is ours.
3. **The new `mixengined` is run once**, before anything is replaced, to be sure this machine will
   start it. An update that would leave you with a daemon that does not run is stopped here rather
   than discovered afterwards.
4. What is running is stopped, the binaries are replaced, and the daemon exits.
5. `mix` starts the new daemon, which starts your services again.

## The one binary this never touches

`mixengine-elevate` runs as an administrator, and replacing it is a privileged act. `mix
self-update` deliberately leaves it exactly as it was.

```bash
mix elevation upgrade
```

That is the separate, deliberate act. It downloads the helper this release publishes, checks
MixEngine's signature on it, runs it once to be sure it starts, and puts the replacement in the
queue. **Nothing is installed by that command**: `mix elevation grant` is what raises the prompt,
and the helper already installed checks the signature again itself before it allows anything to
overwrite it.

Old and new coexist safely in the meantime. The daemon and the helper agree a protocol version when
they talk, and an older helper keeps serving the operations it knows while MixEngine asks you to
upgrade it.

## When a package manager installed MixEngine

`mix self-update` refuses, says so, and names the directory. That is correct rather than unhelpful:
a copy installed by `apt`, `dnf` or a `.pkg` is owned by that package manager, and replacing files
underneath it would leave your system's own records describing something that is no longer there.
Update it the way you installed it.

The portable zip, the AppImage, the Windows per-user installer and a build from source are all
updated by `mix self-update` normally.

## Versions

MixEngine uses semantic versioning, one version across everything it ships. Before 1.0 the API may
break between minor versions, and each break is listed in the changelog — which is what
`mix self-update --check` prints before it asks.
