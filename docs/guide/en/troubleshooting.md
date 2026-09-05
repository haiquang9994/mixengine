+++
title = "When something is wrong"
slug = "troubleshooting"
order = 14
summary = "mix doctor first, then the four commands that answer the questions people actually have — and one file that holds everything a bug report needs."
+++

# When something is wrong

## Start here

```bash
mix doctor
```

It examines the machine and says what is wrong with it. It **reports and repairs nothing** unless
you ask, and it exits non-zero when it found a problem, so a script can ask too.

```bash
mix doctor --repair
```

Repairs everything that can be repaired. Anything inside MixEngine's own home is fixed at once;
anything needing an administrator is queued, shown to you, and then granted in **one** prompt for
the whole batch. `--yes` skips the confirmation before that prompt.

## The four questions people actually have

### "Is anything running?"

```bash
mix status
mix service list
```

`status` is the daemon: its version, its home, and what it is supervising. `service list` is the row
per service with what each one is doing.

### "Why does this name not open?"

```bash
mix domain status blog.test
```

Four facts, answered separately rather than as one verdict: whether the name is declared, how it is
routed, whether it resolves on this machine right now, and whether anything answers on it. The one
that is `no` is the one to fix.

### "Why is the padlock not green?"

```bash
mix cert status
mix cert ca-status
```

`cert status` opens a real connection and reports the certificate that was actually presented, which
is the only thing a browser ever sees. `ca-status` says what the authority is. If the authority is
not trusted, `mix doctor --repair` is what puts it back.

### "Which PHP is this, and why?"

```bash
mix runtime resolve php
```

The version this directory gets, **and which of the four sources decided it** — which is the half
you want when the answer is not the one you expected.

## Reading the logs

```bash
mix service logs caddy --follow
mix service logs mariadb@main -n 200
```

`--follow` survives the service crashing and being restarted: what is being followed is the service,
not one run of its process. The daemon's own log is `logs/daemon.log` inside MixEngine's home.

For a long operation — an install, a blueprint apply — the job is where to look:

```bash
mix job list
mix job status <id>
mix job logs <id>
```

`mix job logs` only answers for a job that runs somebody else's program, which today means a
blueprint running its own scaffold command. Everything else a job does is reported as progress and
as a result, and this says so rather than pretending output was lost.

## Common situations

**A port is already in use.** Something else on your machine has it. `mix service create --port`
picks another for a new service; for one that exists, delete it and create it again on a different
port — the data directory is kept.

**The daemon will not start.** Read `logs/daemon.log` in the home. `mix status --no-autostart` asks
whether one is running without starting one, which is the right question when you are diagnosing
rather than working.

**A command needs a version that is not installed.** MixEngine says so and names the exact
`mix runtime install` command to type. When a *range* was asked for, it cannot know which version
satisfies it and points at `mix runtime available` instead.

**Something asked for an administrator and you said no.** Nothing is half-applied. `mix elevation
status` shows what is still waiting, and `mix elevation grant` asks again.

## Reporting a bug

```bash
mix doctor --bundle
```

One archive with everything a bug report needs: what `doctor` found, this daemon's status, what this
machine is, and the tail of the log. `--out` copies it somewhere of your choosing.

**What it deliberately leaves out is named in the archive itself**, so nobody has to guess whether a
missing section is a redaction or a failure. Open it and look before you send it anywhere — it is a
plain archive, and it is yours.

Every `mix` command also takes `--json`, which is often the fastest way to show somebody exactly
what you saw.
