+++
title = "Installing MixEngine"
slug = "install"
order = 2
summary = "The installer for your system, what it touches, what it deliberately does not, and how to check what you downloaded."
+++

# Installing MixEngine

Every build is published on the project's GitHub releases page, with a checksum and a signature
beside it. Pick the file for your system below. Installing changes as little as it can: nothing is
added to your certificate store, your DNS settings or your firewall until the day you ask for
something that needs it — see [What MixEngine asks permission for](./permissions.md).

## What you are installing

Four programs, and it is worth knowing what each is before one of them surprises you.

| Program | What it does |
| --- | --- |
| `mixengined` | The daemon. It owns everything MixEngine knows and supervises everything it runs. |
| `mix` | The command you type. It asks the daemon and prints the answer. |
| `mixengine-shim` | The stand-in for `php`, `node`, `python` and `ruby` that picks the right version. |
| `mixengine-elevate` | The one program that runs as an administrator, for a few seconds at a time. |

The first three are installed together, as you. The fourth is not placed by the installer at all on
most systems — MixEngine installs it itself, the first time something needs an administrator, inside
a prompt you were going to see anyway.

## Windows

Two files are published, and either is a complete install.

- **`mixengine-<version>-windows-x86_64-setup.exe`** — a per-user installer. It writes into your own
  profile and puts its directory on your `PATH`, so no administrator prompt is involved and neither
  is anybody else's account on the machine.
- **`mixengine-<version>-windows-x86_64.zip`** — the same programs in a folder. Extract it wherever
  you like and run `mix.exe` from there.

Windows ARM builds are published beside them, named `aarch64`.

**Expect a SmartScreen warning.** MixEngine is not signed with an Authenticode certificate, so
Windows shows *"Windows protected your PC"* and hides the button behind **More info → Run anyway**.
That is a statement about a certificate nobody has bought, not about the file: check the signature
below if you want a real answer about what you downloaded. The warning tends to come back with every
release, because reputation with no publisher identity accrues to a file rather than to a project.

## macOS

**`mixengine-<version>-macos-universal.pkg`**, one package for both Intel and Apple silicon.

MixEngine has no Apple Developer ID either, so double-clicking the package in Finder gets you a
Gatekeeper dialog and, on macOS 15 and later, a trip through **System Settings → Privacy & Security
→ Open Anyway**. Installing from a terminal avoids all of that:

```bash
sudo installer -pkg mixengine-0.1.0-macos-universal.pkg -target /
```

That is the instruction to reach for first on a command-line product. The package runs as root, so
it also places the privileged helper for you.

## Linux

Three files, each a complete install:

- **`.deb`** for Debian, Ubuntu and their relatives
- **`.rpm`** for Fedora, RHEL and openSUSE
- **`.AppImage`**, which needs no package manager and no root at all

```bash
sudo dpkg -i mixengine_0.1.0_amd64.deb
sudo rpm -i mixengine-0.1.0.x86_64.rpm
chmod +x MixEngine-0.1.0-x86_64.AppImage && ./MixEngine-0.1.0-x86_64.AppImage
```

Both packages are built against glibc 2.28, so they run on the long-term-support distributions they
are aimed at rather than only on something as new as the machine that built them. `aarch64` builds
are published beside the `x86_64` ones.

## From source

MixEngine is Rust, and nothing else:

```bash
git clone https://github.com/mixnz/mixengine.git
cd mixengine
cargo build --release
```

The binaries land in `target/release/`. This is the fourth way of installing that runs entirely as
you, which is why placing the privileged helper is never a packager's job.

## Checking what you downloaded

Two files sit beside every artifact, and they answer different questions.

```bash
sha256sum -c mixengine-0.1.0-linux-x86_64.tar.gz.sha256
minisign -Vm mixengine-0.1.0-linux-x86_64.tar.gz -P <the key in packaging/updates.pub>
```

The `.sha256` tells you whether two downloads of the same file are the same file. **It is not a
signature** and is not offered as one: anybody who could replace the artifact could replace the
checksum beside it. The `.minisig` is the real answer — an Ed25519 signature MixEngine's own release
pipeline makes, against a public key committed in this project's repository as
`packaging/updates.pub` and compiled into MixEngine itself. That is the same key `mix self-update`
checks before it replaces anything.

## After installing

Open a new terminal — the installer changed your `PATH`, and a shell that was already running has
not heard about it — and ask:

```bash
mix status
```

The first `mix` command starts the daemon if it is not already running. What you should see is a
healthy daemon, its version, and nothing being supervised yet.

Then put the runtime commands on your `PATH`, which is a separate step because it is a separate
directory:

```bash
mix path install
```

That fills `<root>/bin` with the shims that make `php`, `node`, `python` and `ruby` resolve to the
version each directory asks for, rather than to one version for the whole machine.

## What the installer did not do

Nothing outside your own account, and nothing to the rest of the machine:

- **No certificate authority** was installed. That happens the first time you ask for HTTPS.
- **No DNS or hosts change** was made. That happens the first time you create a site.
- **No firewall rule** and **no port grant**. Those happen when a site needs them.
- **No runtime and no server** was downloaded. MixEngine installs PHP, MariaDB and the rest on
  request, and only the versions you ask for.
- **Nothing was registered to start at login.** `mix autostart enable` is how that becomes true.

Every one of those is described in [What MixEngine asks permission for](./permissions.md), including
what each prompt will literally change before you agree to it.

Ready? [Your first site](./getting-started.md) takes about five minutes.
