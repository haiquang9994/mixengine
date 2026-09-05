+++
title = "Command reference"
slug = "cli"
order = 15
summary = "Every mix command and every flag, generated from the binary's own definitions."
+++

# Command reference

Every command `mix` accepts, in version 0.1.0. This page is **generated** from the
binary's own definitions, so it cannot describe a flag that is not there — and it is the
one page of this handbook that exists in English only, because those definitions are.
`mix docs cli --lang vi` says why, in Vietnamese.

The same text is `mix <command> --help` on the machine in front of you, and
`mix docs --reference` prints this whole page.

Three flags are accepted by every command below and are not repeated in each table:
`--home <DIR>` chooses which installation to talk to, `--json` asks for the answer as
JSON, and `--no-autostart` refuses to start a daemon that is not running.

## mix status

Show the daemon's health, version and what it is currently running

```
mix status
```

## mix daemon

Control the daemon itself

```
mix daemon <COMMAND>
```

### mix daemon stop

Stop the services this home is running, then stop the daemon

```
mix daemon stop
```

## mix docs

Read the MixEngine handbook, offline, in English or Vietnamese.

With no topic it lists them. It talks to no daemon and needs no home — the pages are compiled into
this binary, which is what makes `mix docs install` answer on a machine where nothing starts. The
same pages are published at https://mixnz.github.io/mixengine/, as HTML for a person and as plain
Markdown for a program.

```
mix docs [TOPIC] [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<TOPIC>` | Which topic. Omit it to list them |
| `--lang` `<CODE>` | Which language: `en` or `vi`. An unrecognised one is answered in English |
| `--reference` | Print the whole command reference as Markdown, instead of a topic. This is what `docs/guide/en/cli.md` is generated from, by `packaging/docs.sh --reference` — so the reference cannot describe a flag this binary does not have. It is English only, because the definitions it is generated from are. It does not conflict with `--lang`: that flag carries `MIXENGINE_LANG`, and a variable somebody exported once should not be able to refuse a command. |

## mix runtime

Install, remove and choose between language runtimes

```
mix runtime <COMMAND>
```

### mix runtime list

List the runtimes installed in this home

```
mix runtime list [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `--kind` `<RUNTIME>` | Only this language. Every one of them when it is left out |

### mix runtime available

List the versions the package index offers for this machine

```
mix runtime available [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `--kind` `<RUNTIME>` | Only this language. Every one of them when it is left out |

### mix runtime install

Download and install one version

```
mix runtime install <RUNTIME> <VERSION> [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<RUNTIME>` | Which language |
| `<VERSION>` | Which version, exactly as `mix runtime available` lists it. Required, and deliberately not a constraint like `8.3`, even now that the daemon can read one: choosing a version from a range is *resolution*, it answers with what is installed, and none of these three commands is asking that question — an install picking `8.3`'s newest would be picking between versions none of which are here yet. `mix runtime resolve` is where a range belongs. |
| `--no-wait` | Return once the daemon has accepted the install, rather than once it has finished. `mix` waits by default, because `mix runtime install php 8.3.33 && …` is a sentence about PHP being there. What comes back instead is the job, which `mix job wait` can be pointed at later. |

### mix runtime uninstall

Remove one installed version.

Refused while a registered project pins it, naming the projects, and while the php-fpm pool that
runs out of it is running. `--force` crosses the first and never the second.

```
mix runtime uninstall <RUNTIME> <VERSION> [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<RUNTIME>` | Which language |
| `<VERSION>` | Which version, exactly as `mix runtime available` lists it. Required, and deliberately not a constraint like `8.3`, even now that the daemon can read one: choosing a version from a range is *resolution*, it answers with what is installed, and none of these three commands is asking that question — an install picking `8.3`'s newest would be picking between versions none of which are here yet. `mix runtime resolve` is where a range belongs. |
| `--force` | Remove it even though a registered project pins it |

### mix runtime default

Make one installed version the one its kind resolves to

```
mix runtime default <RUNTIME> <VERSION>
```

| Flag | What it does |
| --- | --- |
| `<RUNTIME>` | Which language |
| `<VERSION>` | Which version, exactly as `mix runtime available` lists it. Required, and deliberately not a constraint like `8.3`, even now that the daemon can read one: choosing a version from a range is *resolution*, it answers with what is installed, and none of these three commands is asking that question — an install picking `8.3`'s newest would be picking between versions none of which are here yet. `mix runtime resolve` is where a range belongs. |

### mix runtime ext

Which extensions an installed build loads.

Under `runtime` rather than as `mix php ext …`, which is what `.claude/features/runtime-versions.md`
wrote: a per-language command family for one language is a noun this CLI would then owe every other
runtime.

```
mix runtime ext <COMMAND>
```

#### mix runtime ext list

List what this build has, and why each is on or off

```
mix runtime ext list [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `--php` `<VERSION>` | The version, exactly as it is installed. Defaults to the one `php` resolves to here |

#### mix runtime ext enable

Load one on every PHP process of this version

```
mix runtime ext enable <EXTENSION> [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<EXTENSION>` | The extension, as the listing spells it |
| `--php` `<VERSION>` | The version, exactly as it is installed. Defaults to the one `php` resolves to here |

#### mix runtime ext disable

Stop loading one

```
mix runtime ext disable <EXTENSION> [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<EXTENSION>` | The extension, as the listing spells it |
| `--php` `<VERSION>` | The version, exactly as it is installed. Defaults to the one `php` resolves to here |

### mix runtime resolve

Say which installed version a directory uses, and why that one.

The question `php -v` answers by running, asked without running anything — and the reason is the
point of it: what a person wants when the version surprises them is which of the four sources
decided it.

```
mix runtime resolve <RUNTIME> [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<RUNTIME>` | Which language |
| `--version` `<VERSION>` | Use this version or range instead of what the directory says. Exact (`8.3.33`), a series (`8.3`, `8`) or a caret (`^8.3`), resolved against what is installed and never against what could be downloaded. |
| `--cwd` `<DIR>` | Resolve as if this were the working directory |

## mix package

Install and remove the servers, databases and caches a service runs

```
mix package <COMMAND>
```

### mix package list

List the packages installed in this home

```
mix package list [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `--package` `<PACKAGE>` | Only this package. Every one of them when it is left out |

### mix package available

List the versions the package index offers for this machine.

Only packages this build knows how to configure and run: an entry MixEngine has no recipe for would
unpack into a directory nothing could start.

```
mix package available [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `--package` `<PACKAGE>` | Only this package. Every one of them when it is left out |

### mix package install

Download and install one version

```
mix package install <PACKAGE> <VERSION> [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<PACKAGE>` | Which package, as `mix package available` lists it |
| `<VERSION>` | Which version, exactly as `mix package available` lists it |
| `--no-wait` | Return once the daemon has accepted the install, rather than once it has finished |

### mix package uninstall

Remove one installed version.

Refused while a service is an instance of it, naming the services — `mix service delete` is what
frees it, and deleting a service keeps its data directory.

```
mix package uninstall <PACKAGE> <VERSION>
```

| Flag | What it does |
| --- | --- |
| `<PACKAGE>` | Which package, as `mix package available` lists it |
| `<VERSION>` | Which version, exactly as `mix package available` lists it |

## mix project

Register the directories this home knows about, and what they pin

```
mix project <COMMAND>
```

### mix project create

Register a directory as a project.

With no `--name` and no `--pin`, whatever the `mixengine.toml` in that directory says is used —
which is what adopting a colleague's checkout is.

```
mix project create [DIR] [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<DIR>` | The project's root. Defaults to the current directory |
| `--name` `<NAME>` | What to call it. Defaults to the manifest's name, then to the directory's own |
| `--pin` `<RUNTIME=VERSION>` | Pin a language, as `php=^8.3`. May be given more than once |

### mix project list

List the projects this home has been told about

```
mix project list
```

### mix project show

Show one, with its pins in the order they take effect

```
mix project show [PROJECT]
```

| Flag | What it does |
| --- | --- |
| `<PROJECT>` | The project's name. Defaults to whichever project the current directory is in |

### mix project update

Change a project's name, root or pins.

`--pin` **replaces** every pin rather than adding to one: `--clear-pins` with no `--pin` removes
them all, and leaving both out changes nothing.

```
mix project update [PROJECT] [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<PROJECT>` | The project's name. Defaults to whichever project the current directory is in |
| `--name` `<NAME>` | A new name. `id` spelled out because the flattened project argument is also called `name`, and clap refuses two arguments under one id — it did so at *parse* time, so `mix project update blog --name blogging` panicked instead of running. Found by T77's `every_command_is_one_clap_can_build`, which is now what stops the next one. |
| `--root` `<DIR>` | A new root, for a repository that moved |
| `--pin` `<RUNTIME=VERSION>` | Pin a language, as `php=^8.3`. Replaces every pin the project had |
| `--clear-pins` | Remove every pin |

### mix project keep-warm

Hold this project's services out of idle shutdown while you are working on it.

A verb of its own rather than a flag on `update`, because it is a thing you do to a project for an
afternoon and not part of what the project *is*.

It reaches the PHP pool this project's sites name. It does not yet reach the database they query —
nothing in MixEngine records which database a project uses.

```
mix project keep-warm [PROJECT] [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<PROJECT>` | The project's name. Defaults to whichever project the current directory is in |
| `--off` | Stop keeping it warm |

### mix project delete

Forget a project. The directory is left exactly as it is

```
mix project delete [PROJECT]
```

| Flag | What it does |
| --- | --- |
| `<PROJECT>` | The project's name. Defaults to whichever project the current directory is in |

### mix project export

Write the project into `<root>/mixengine.toml`, keeping everything else in the file

```
mix project export [PROJECT]
```

| Flag | What it does |
| --- | --- |
| `<PROJECT>` | The project's name. Defaults to whichever project the current directory is in |

## mix site

Declare what is served out of a project's directory, and at what name

```
mix site <COMMAND>
```

### mix site create

Declare a site under a project.

With nothing but a project named, whatever the `[site]` and `[[services]]` in that project's
`mixengine.toml` say is used — which is what adopting a colleague's site is.

```
mix site create [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `--project` `<PROJECT>` | The project. Defaults to whichever project the current directory is in |
| `--domain` `<DOMAIN>` | A domain. The first is the primary; repeat for aliases. Defaults to `<project>.test` |
| `--doc-root` `<DIR>` | What is served, relative to the project's root. Defaults to the root itself |
| `--kind` `<KIND>` | What serves it |
| `--upstream` `<URL>` | Where a `reverse-proxy` forwards to |
| `--port` `<PORT>` | The port a `node-app` listens on |
| `--pool` `<SERVICE>` | The php-fpm pool a `php-fpm` site uses. Defaults to whatever this directory resolves to |
| `--service` `<SERVICE>` | A service the site declares, as `mariadb@main`. May be given more than once |
| `--https` `<HTTPS>` | Declare HTTPS for it. Phase 5 is what acts on this |
| `--i-know` | Accept a `.local` domain, which belongs to mDNS |

### mix site list

List the sites this home has been told about

```
mix site list [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `--project` `<PROJECT>` | Only this project's |

### mix site show

Show one, with its domains, its pool and its services

```
mix site show [DOMAIN]
```

| Flag | What it does |
| --- | --- |
| `<DOMAIN>` | Any of the site's domains. Defaults to the site of whichever project you are in |

### mix site update

Change what a site is.

`--domain` and `--service` **replace** rather than add to what the site had: giving neither changes
neither.

```
mix site update [DOMAIN] [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<DOMAIN>` | Any of the site's domains. Defaults to the site of whichever project you are in |
| `--domain` `<DOMAIN>` | A domain. The first is the primary; repeat for aliases. Replaces the whole list |
| `--doc-root` `<DIR>` | A new doc root |
| `--kind` `<KIND>` | A new kind |
| `--upstream` `<URL>` | Where a `reverse-proxy` forwards to |
| `--port` `<PORT>` | The port a `node-app` listens on |
| `--pool` `<SERVICE>` | The php-fpm pool |
| `--service` `<SERVICE>` | A service the site declares. Replaces the whole list |
| `--https` `<HTTPS>` | Whether HTTPS is declared |
| `--state` `<STATE>` | Serve it, or stop serving it |
| `--i-know` | Accept a `.local` domain |

### mix site share

Let the local network reach this site, and print a QR code for it.

This site only: every other site keeps answering on loopback alone. The certificate gains the LAN
address, and one administrator prompt asks for the firewall rule.

```
mix site share [DOMAIN] [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<DOMAIN>` | Any of the site's domains. Defaults to the site of whichever project you are in |
| `--interface` `<NAME>` | Which network to share on, by the name this machine gives it. Needed only where more than one is up — MixEngine refuses to choose rather than putting a site on a network you did not mean, and names the candidates when it does. |
| `--for` `<LENGTH>` | How long to share for: `30s`, `90m`, `2h`, `1d`, or a bare number of seconds. Measured from when the share began, so asking for a length shorter than the site has already been shared for is refused rather than ending it on the spot. Off by default: a share with no `--for` lasts until you unshare it or this machine leaves the network. |

### mix site unshare

Take it back off the local network.

Removes the firewall rule, rebinds to loopback and reissues the certificate without the address. A
site that is not shared is left as it is.

```
mix site unshare [DOMAIN]
```

| Flag | What it does |
| --- | --- |
| `<DOMAIN>` | Any of the site's domains. Defaults to the site of whichever project you are in |

### mix site start

Serve this site.

A flag and a re-render: the front end is told to read its configuration again. Nothing is started —
a site is not a process, and the services it uses have states of their own.

```
mix site start [DOMAIN]
```

| Flag | What it does |
| --- | --- |
| `<DOMAIN>` | Any of the site's domains. Defaults to the site of whichever project you are in |

### mix site stop

Stop serving this site, keeping the declaration

```
mix site stop [DOMAIN]
```

| Flag | What it does |
| --- | --- |
| `<DOMAIN>` | Any of the site's domains. Defaults to the site of whichever project you are in |

### mix site delete

Forget a site. The files are left exactly as they are

```
mix site delete [DOMAIN]
```

| Flag | What it does |
| --- | --- |
| `<DOMAIN>` | Any of the site's domains. Defaults to the site of whichever project you are in |

## mix blueprint

Write down what a project is made of, and see what applying that somewhere else would do

```
mix blueprint <COMMAND>
```

### mix blueprint capture

Write down what a project is made of

```
mix blueprint capture <NAME> [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<NAME>` | What to file it under: lower-case letters, digits and hyphens. Positional rather than `--name`, because the flattened project argument is already called `name` and clap refuses two arguments under one id — found by running the command rather than by a test, which is why it is worth a sentence here. |
| `--project` `<PROJECT>` | Which project. Defaults to whichever project the current directory is in |
| `--description` `<TEXT>` | What it is for |
| `--overwrite` | Replace the blueprint already filed under this name |

### mix blueprint import

Take in a blueprint somebody else wrote.

**What arrives without a signature the gallery key vouches for is untrusted for good** — nothing
raises that afterwards, and it is what decides how loudly its `[scaffold]` command has to be agreed
to before it runs.

```
mix blueprint import <FILE> [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<FILE>` | The manifest to read |
| `--name` `<NAME>` | What to file it under. Defaults to the file's own name, without `.toml` |
| `--signature` `<FILE>` | The detached signature to check it against. Defaults to `<FILE>.minisig` if that exists |
| `--overwrite` | Replace the blueprint already filed under that name |

### mix blueprint list

Every blueprint this home holds

```
mix blueprint list
```

### mix blueprint apply

What applying one would do

```
mix blueprint apply <BLUEPRINT> [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<BLUEPRINT>` | Which blueprint |
| `--project` `<NAME>` | What the new project is called, and what `{project}` becomes |
| `--path` `<DIR>` | Where it goes. Defaults to `<current directory>/<project>` |
| `--dry-run` | Stop after planning, and print the plan. Sent as it is typed rather than insisted on here: whether this build can carry an apply out is the daemon's to say, and a client that refused to ask would be holding a rule of its own. |
| `--install-missing` | Answer every version question by installing what the blueprint asks for |
| `--use-installed` | Answer every version question by using what this machine already has |
| `--run-scaffold` | Run the blueprint's own `[scaffold]` command without asking first. For a blueprint the gallery signed. An unsigned one takes the other flag, and neither covers the other: a script that runs somebody's unsigned command should say so on the line that does it. |
| `--run-untrusted-scaffold` | Run an **untrusted** blueprint's own `[scaffold]` command without asking first. Nothing vouches for what this runs. The command is still printed before it starts. |
| `--grant` | Spend the one elevation prompt at the end without asking first |

## mix extension

Read an `extension.toml` without installing anything

```
mix extension <COMMAND>
```

### mix extension inspect

Say what installing this extension here would produce

```
mix extension inspect <PATH>
```

| Flag | What it does |
| --- | --- |
| `<PATH>` | The extension's directory, or its `extension.toml` |

### mix extension list

What this home has installed

```
mix extension list
```

### mix extension available

What the signed registry publishes

```
mix extension available
```

### mix extension plan

Say what installing one would do, and change nothing

```
mix extension plan [ID] [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<ID>` | The extension's id in the registry |
| `--path` `<PATH>` | A directory to read instead of the registry. **Nothing vouches for one of these.** |

### mix extension install

Install one

```
mix extension install [ID] [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<ID>` | The extension's id in the registry |
| `--path` `<PATH>` | A directory to install instead of a registry entry. **Nothing vouches for one of these**, and the row records it as unsigned for as long as it stays installed |
| `--yes` | Install without asking about what it declares |
| `--no-wait` | Answer with the job rather than waiting for it |

### mix extension uninstall

Remove one

```
mix extension uninstall <ID> [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<ID>` | Which extension |
| `--delete-data` | Delete its data directory as well. **Kept when this is absent**, which is the answer that can be undone. |

### mix extension start

Start the service an extension runs as

```
mix extension start <ID>
```

| Flag | What it does |
| --- | --- |
| `<ID>` | Which extension |

### mix extension stop

Stop it

```
mix extension stop <ID>
```

| Flag | What it does |
| --- | --- |
| `<ID>` | Which extension |

## mix database

Make a database on one of this home's database servers, and an account that reaches it

```
mix database <COMMAND>
```

### mix database create

Make a database and the account that reaches it.

The instance is started if it is not running. Nothing prints the password: it is put in this
machine's credential store, and what is printed is where.

```
mix database create <SERVICE> [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<SERVICE>` | Which instance: `mariadb@main`, `postgres@shop` |
| `--name` `<NAME>` | The database's name |
| `--user` `<ACCOUNT>` | The account's name. The database's own when nobody says |

### mix database client

Where this instance could be opened, and with what.

Reads only: starts nothing, opens nothing. "Not installed" is an answer, not a failure.

```
mix database client <SERVICE>
```

| Flag | What it does |
| --- | --- |
| `<SERVICE>` | Which instance: `mariadb@main`, `redis@main` |

### mix database open

Open this instance in the installed desktop database client.

The instance is started if it is not running. The account's password is read from this machine's
credential store at that moment and handed to the client in its own environment — never printed,
never put in an argument. Exits 1 when no client is installed, and says what to install.

```
mix database open <SERVICE> [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<SERVICE>` | Which instance |
| `--user` `<ACCOUNT>` | The account to sign in as. The server's administrator when nobody says |
| `--database` `<NAME>` | A database to open at |

## mix metrics

Show what MixEngine is costing this machine: CPU and memory, per service and for the daemon.

One reading and out by default. `--watch` opens the live stream, which is also what puts the daemon
on its one-second rate — it samples once a minute when nobody is looking.

```
mix metrics [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `--watch` | Keep printing, a block per reading, until interrupted |
| `--since` `<SINCE>` | Read the recorded history instead, starting this far back: `30m`, `2h`, `1d` |
| `--service` `<SERVICE>` | One subject only. Omit for every service and the daemon |

## mix doctor

Examine this machine and say what is wrong with it.

Reports and repairs nothing unless `--repair` is passed. Exits non-zero when it found a problem, so
a script can ask.

```
mix doctor [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `--repair` | Repair everything that can be repaired, and ask for the rest. Repairs inside this home are made at once. Anything needing an administrator is queued, shown, and then granted once — one prompt for the whole batch. |
| `--yes` | Do not ask before raising the prompt. Only with `--repair` |
| `--no-wait` | Return as soon as the grant has started, rather than waiting for it. Only with `--repair` |
| `--bundle` | Write one diagnostics archive and print where it went. Everything a bug report needs in one file: the findings above, this daemon's status, what this machine is, and the tail of the log — with whatever was deliberately left out named beside them. |
| `--out` `<FILE>` | Copy the archive here as well. Only with `--bundle` |

## mix self-update

Update MixEngine itself.

Checks for a newer release and shows its version, its size and what changed before asking. On yes,
the daemon downloads it, checks the signature, runs the new `mixengined` once to be sure this
machine will start it, stops what it is supervising, replaces the binaries and exits — and this
command starts the new daemon, which starts your services again.

`mixengine-elevate` is never replaced here. It runs as root, and updating it needs an elevation
prompt of its own.

A copy of MixEngine that a package manager installed is not updated by this: it says so, and names
the directory.

```
mix self-update [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `--check` | Check and print what is available. Installs nothing |
| `--yes` | Answer the prompt in advance, for a script with nobody at the keyboard |

## mix uninstall

Take MixEngine off this machine.

Undoes everything MixEngine has written outside its own directory — the hosts block, the DNS
routing, the port grant, the certificate authority, the firewall rules, the login entry, your PATH
entry, the privileged helper and its audit log — and then removes the directory itself.

`--dry-run` names every one of them and changes nothing. Exits non-zero when anything it acted on is
still there, so a script can ask.

```
mix uninstall [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `--dry-run` | List what would be removed, and remove nothing |
| `--keep-home` | Leave this home's directory where it is, and undo only what is outside it. Keeps the databases in `data/`, the certificates and everything else this home holds. The daemon keeps running, because there is still a home for it to serve. |
| `--yes` | Answer the confirmation in advance, for a script with nobody at the keyboard |
| `--no-wait` | Start the work and print the job, rather than waiting for it to finish |

## mix domain

Add, remove and diagnose the names this home answers for

```
mix domain <COMMAND>
```

### mix domain add

Give a site one more name.

The new name is an alias: the site's primary domain is unchanged, because that is what its canonical
URL and — from the HTTPS work — its certificate are named after.

```
mix domain add <DOMAIN> [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<DOMAIN>` | The name to add |
| `--site` `<DOMAIN>` | Any of the site's existing domains |
| `--i-know` | Accept `.local`, which belongs to mDNS and works until somebody plugs in a printer |

### mix domain remove

Take one name away.

Refused for a site's last domain and for its primary; `mix site update` reorders, and the first
`--domain` it is given becomes the primary.

```
mix domain remove <DOMAIN>
```

| Flag | What it does |
| --- | --- |
| `<DOMAIN>` | The name to take away. It names its own site |

### mix domain status

What actually happens to a name, as four facts that can fail one at a time

```
mix domain status [DOMAIN]
```

| Flag | What it does |
| --- | --- |
| `<DOMAIN>` | One name, or every name this home declares |

## mix service

Inspect and control the services this home declares

```
mix service <COMMAND>
```

### mix service list

List every declared service and what it is doing

```
mix service list
```

### mix service status

Describe one service.

The id is required, where `start` and the rest take an optional one: a status with no subject is a
`list` that was typed wrongly, and answering it as a list would hide that.

```
mix service status <SERVICE>
```

| Flag | What it does |
| --- | --- |
| `<SERVICE>` | The service to describe |

### mix service logs

Print what a service has been printing.

The one `mix service` subcommand that is not a `service.*` method: output is a stream, and a
JSON-RPC call cannot be one, so the lines arrive on a connection of their own.

```
mix service logs <SERVICE> [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<SERVICE>` | The service to read |
| `-n`, `--lines` `<LINES>` | How many of the lines already printed to begin with |
| `-f`, `--follow` | Keep printing as the service prints, rather than stopping at what it already said. Survives the service crashing and being restarted: what is being followed is the service, not one run of its process. |

### mix service limits

What this service may take, and what this machine will actually enforce of it.

With no subcommand: read it. `set` replaces it, `clear` removes it.

```
mix service limits <SERVICE> <COMMAND>
```

| Flag | What it does |
| --- | --- |
| `<SERVICE>` | The service to read or cap |

#### mix service limits set

Replace every limit on this service.

**Every field, not only the ones named.** A flag left out is that field's default — uncapped, or
ordinary priority — so `set --cpu 50` clears a memory ceiling that was there. That is deliberate:
composing a partial change would mean reading the current value and merging it, which is business
logic a client may not hold. What this does instead is print all three fields of the result, so a
cleared limit is on the screen.

```
mix service limits set [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `--cpu` `<PERCENT>` | A ceiling on CPU, as a percentage of one core. Left out: uncapped |
| `--memory` `<MB>` | A ceiling on memory, in megabytes. Left out: uncapped |
| `--priority` `<PRIORITY>` | How this service competes for CPU |

#### mix service limits clear

Remove every limit from this service.

A named operation rather than a `set` with three absent flags, so that "uncap this" is something a
person can type rather than something they have to infer.

```
mix service limits clear
```

### mix service idle

When this service is stopped for being unused, and what is holding it open.

With no flag: read it. One of the three flags replaces it.

Nothing idles by default in this build: a stopped service stays stopped until you start it, so
switching this on is a choice you make per service.

```
mix service idle <SERVICE> [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<SERVICE>` | The service to read or set |
| `--after` `<DURATION>` | Stop it once nothing has used it for this long — `30m`, `2h`, `90m` |
| `--never` | Never stop it for being unused, whatever a later release makes the default |
| `--default` | Go back to whatever its recipe wants, which in this build is never |

### mix service create

Create a service from an installed package.

The part of the id before `@` is the package it is an instance of, which is why there is no separate
argument for it: `mariadb@main` is an instance of `mariadb`, and a package that runs only once —
Caddy — is named without an `@` at all.

```
mix service create <SERVICE> <VERSION> [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<SERVICE>` | The service to create |
| `<VERSION>` | Which installed version of its package to run |
| `--port` `<PORT>` | The port it listens on. The recipe's own default when it is left out |
| `--bind` `<ADDR>` | The address it binds. `127.0.0.1` when it is left out |
| `--data-dir` `<DIR>` | Where its data lives. The home's own layout when it is left out, and never a directory another service already keeps its data in |
| `--autostart` | Start it whenever the daemon starts |

### mix service delete

Delete a service, keeping its data directory.

Takes the row and the configuration generated from it. **Never the data** — that is somebody's
databases, and the answer names the directory that was left so nobody has to go looking.

```
mix service delete <SERVICE> [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<SERVICE>` | The service to delete |
| `--force` | Delete it even though a site declares it |

### mix service start

Start a service, and everything it depends on

```
mix service start [SERVICE] [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<SERVICE>` | The service to act on. Every declared service when it is left out. Naming one does not mean acting on one — a plan is the transitive set — and what the daemon walked comes back in the answer. |
| `--no-wait` | Return once the daemon has accepted the plan, rather than once it has walked it. `mix` waits by default, because `mix service start db && …` is a sentence about the database being up: an answer sent before the walk would exit `0` for a service that never came up. |

### mix service stop

Stop a service, and everything that depends on it

```
mix service stop [SERVICE] [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<SERVICE>` | The service to act on. Every declared service when it is left out. Naming one does not mean acting on one — a plan is the transitive set — and what the daemon walked comes back in the answer. |
| `--no-wait` | Return once the daemon has accepted the plan, rather than once it has walked it. `mix` waits by default, because `mix service start db && …` is a sentence about the database being up: an answer sent before the walk would exit `0` for a service that never came up. |

### mix service restart

Stop a service and what depends on it, then start that same set again

```
mix service restart [SERVICE] [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<SERVICE>` | The service to act on. Every declared service when it is left out. Naming one does not mean acting on one — a plan is the transitive set — and what the daemon walked comes back in the answer. |
| `--no-wait` | Return once the daemon has accepted the plan, rather than once it has walked it. `mix` waits by default, because `mix service start db && …` is a sentence about the database being up: an answer sent before the walk would exit `0` for a service that never came up. |

## mix job

Watch the long operations this daemon is running

```
mix job <COMMAND>
```

### mix job list

List what this home has run, newest first

```
mix job list [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `--state` `<STATE>` | Only jobs in this state |
| `-n`, `--limit` `<COUNT>` | At most this many |

### mix job status

Describe one job

```
mix job status <JOB>
```

| Flag | What it does |
| --- | --- |
| `<JOB>` | The job, as `mix job list` numbers them |

### mix job wait

Wait for a job to finish.

**Answers when the job ends or when the wait runs out**, and the second is not an error: what comes
back is the job as it stands. The exit status is what a script branches on — non-zero for a job that
failed, and for one that has not finished yet.

```
mix job wait <JOB> [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<JOB>` | The job to wait for |
| `--timeout` `<SECONDS>` | How long to wait. The daemon caps what it grants |

### mix job cancel

Ask a running job to stop.

Cancellation is cooperative, so what comes back may still say `running`: the work ends when it next
looks. Cancelling a job that has already ended is not an error.

```
mix job cancel <JOB>
```

| Flag | What it does |
| --- | --- |
| `<JOB>` | The job to cancel |

### mix job logs

What a job printed — roadmap task **T78a**.

**Only a job that runs somebody else's program prints anything**, which today is an apply running a
blueprint's own `[scaffold]` command. Everything else a job does is reported as progress and as its
result, and this answers nothing for those rather than pretending output was lost.

The lines live in memory for as long as the daemon keeps the job's log, so this is what to read
while one runs rather than a record to come back to a week later.

```
mix job logs <JOB> [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<JOB>` | The job, as `mix job list` numbers them |
| `-f`, `--follow` | Keep printing as the job prints |
| `-n`, `--lines` `<COUNT>` | How many of the lines already printed to begin with |

## mix path

Put this home's commands on your PATH, or take them off again

```
mix path <COMMAND>
```

### mix path status

Say whether a new terminal would find this home's commands

```
mix path status
```

### mix path install

Fill `<root>/bin` and put it on this user's PATH.

Idempotent, and it says which of the two it did: a profile that already carries the line is left
exactly as it is.

```
mix path install
```

### mix path uninstall

Take `<root>/bin` back off this user's PATH.

The commands stay in the directory — they are inside the home, and removing the home is what removes
them.

```
mix path uninstall
```

## mix autostart

Start this home's daemon when you log in, or stop doing that

```
mix autostart <COMMAND>
```

### mix autostart status

Say whether this home's daemon starts when you log in

```
mix autostart status
```

### mix autostart enable

Register it.

Does **not** start the daemon: there is one running, and it is the one answering this. What it
changes is what happens at your next login. Idempotent, and it says which of the two it did.

```
mix autostart enable
```

### mix autostart disable

Remove it.

Does **not** stop the daemon that is running — turning off "start at login" is not a request to lose
the daemon you are using.

```
mix autostart disable
```

## mix elevation

See what needs an administrator's permission, ask for it once, or forget it

```
mix elevation <COMMAND>
```

### mix elevation status

Say what is waiting for permission, and what each of them will change

```
mix elevation status
```

### mix elevation grant

Ask once, for everything that is waiting.

One prompt for the whole queue: `.claude/decisions/0005-on-demand-elevation.md` calls asking inside
a loop a defect. Saying no is a normal answer — the list stays, and this command can be run again
later.

```
mix elevation grant [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `--yes` | Say yes in advance, instead of being asked. What it skips is the question, never the screen: every operation and what it will change is printed either way. It exists for the caller that cannot be asked — a script, a CI step, anything with no terminal behind it — and for `--json`, which has no way to answer. |
| `--no-wait` | Answer as soon as the prompt has been raised, without waiting for it |

### mix elevation upgrade

Fetch the privileged helper this release publishes, and queue its installation.

`mixengine-elevate` runs as root and is deliberately never replaced by `mix self-update`, so this is
the one part of an upgrade that has to be asked for separately.

**Nothing is installed by this command.** It downloads the helper, checks MixEngine's signature on
it, runs it once to be sure this machine will start it, and puts the replacement in the queue — `mix
elevation grant` is what raises the prompt, and the helper already installed checks that signature
again itself before it replaces anything.

```
mix elevation upgrade
```

### mix elevation drop

Forget an operation that is waiting, so it is never asked about again

```
mix elevation drop [OP] [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `<OP>` | Which one, as `mix elevation status` numbers them |
| `--all` | Forget all of them. Its own flag rather than "drop with nothing named": emptying the queue by typing less is exactly the mistake worth making impossible. |

## mix cert

Look at the certificate authority this home signs its sites with

```
mix cert <COMMAND>
```

### mix cert issue

Say what this home's certificate authority is: its name, its fingerprint, how long it has.

**Not whether this machine trusts it.** That is a question about the operating system's own
certificate stores rather than about the authority, this build does not yet ask it, and nothing
printed here implies an answer to it. Give a site the certificate its names need, or every HTTPS
site one.

Idempotent: a certificate that still covers the right names, has more than thirty days left and was
signed by the authority this home has now is left exactly as it is.

```
mix cert issue [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `--site` `<DOMAIN>` | One site, by any of its domains. Every HTTPS site when this is left out |

### mix cert status

Say whether each site's padlock is green, by asking the server rather than the disk.

Opens a real TLS connection to this home's front end for every site and reports the certificate it
presents — which is the only thing a browser ever sees, and the only way to notice a server still
holding a certificate that was replaced underneath it.

Reads only. Nothing is issued, nothing is installed and nothing is reloaded.

```
mix cert status [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `--site` `<DOMAIN>` | One site, by any of its domains. Every site when this is left out |

### mix cert ca-status

```
mix cert ca-status
```

### mix cert ca-rotate

Replace this home's certificate authority with a new one.

Destructive: every browser holding a cached chain under the old authority stops accepting it, and
every site's certificate is reissued. Nothing is replaced unless this machine can be made to trust
the new one — declining the prompt leaves this home exactly as it was.

```
mix cert ca-rotate [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `--yes` | Answer the confirmation in advance, for a script with nobody at the keyboard |
| `--no-wait` | Start the work and print the job, rather than waiting for it to finish |

### mix cert ca-uninstall

Take this home's certificate authority out of every store that trusts it.

Leaves the certificate and its key on disk, and leaves every site's certificate alone — `mix doctor
--repair` puts the trust back. Removing it from the system store needs an administrator; the browser
databases do not.

```
mix cert ca-uninstall [OPTIONS]
```

| Flag | What it does |
| --- | --- |
| `--yes` | Answer the confirmation in advance, for a script with nobody at the keyboard |
| `--no-wait` | Start the work and print the job, rather than waiting for it to finish |

