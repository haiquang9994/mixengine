+++
title = "Blueprints"
slug = "blueprints"
order = 9
summary = "Write down what a project is made of, and set the same thing up again somewhere else — or on somebody else's machine."
+++

# Blueprints

A blueprint is a written record of what a project is made of: which PHP it wants, which services it
uses, what its site looks like, and optionally a command that scaffolds a fresh copy. It is how you
set the same environment up twice — on a second machine, for a colleague, or for the next project of
the same shape.

## Capturing one

```bash
cd ~/code/blog
mix blueprint capture blog-stack --description "PHP 8.3, MariaDB, Redis"
mix blueprint list
```

The name is what it is filed under — lower-case letters, digits and hyphens.

**What a blueprint carries is the shape, not the contents.** It records that the project uses a
MariaDB and which version; it does not record your data, and it never contains a password. Applying
one gives you the same environment, not a copy of your work.

## Applying one

```bash
mix blueprint apply blog-stack --project shop --dry-run
mix blueprint apply blog-stack --project shop
```

**Run the dry run first.** It prints the plan and changes nothing: which runtimes would be
installed, which services would be created, what the site would be called, and — where there is one
— the scaffold command that would be executed. Nothing about an apply is hidden from that plan.

`--path` says where the new project goes; it defaults to a directory named after the project, under
where you are.

### Answering the version questions

A blueprint asking for PHP 8.3 on a machine that has 8.2 is a question, not a failure. Two flags
answer it up front for every such question in the plan:

| Flag | Means |
| --- | --- |
| `--install-missing` | Install exactly what the blueprint asks for |
| `--use-installed` | Use what this machine already has |

## Importing somebody else's

```bash
mix blueprint import ./blog-stack.toml
```

A blueprint from elsewhere may carry a detached signature — `mix` looks for `<file>.minisig` beside
it, or takes `--signature`. And here is the rule that matters:

**What arrives without a signature the gallery vouches for is untrusted for good.** Nothing raises
that afterwards. Importing it again with a signature does not launder it; the trust state is decided
once, at import, and every listing that names the blueprint shows it.

That state is not decoration. It decides how loudly the blueprint's `[scaffold]` command has to be
agreed to before it runs.

## The scaffold command, and why it is asked about

A blueprint may carry a command to run once in the new project — `composer create-project …`, or the
equivalent for whatever framework it is for. That is somebody else's program running on your
machine, so MixEngine prints the exact command and asks before running it, and it asks differently
depending on where the blueprint came from.

Two flags skip the question, and **neither covers the other**:

| Flag | For |
| --- | --- |
| `--run-scaffold` | A blueprint the gallery signed |
| `--run-untrusted-scaffold` | An untrusted one. Nothing vouches for what this runs |

A script that runs somebody's unsigned command should say so on the line that does it. That is the
entire reason there are two flags rather than one, and the command is printed before it starts in
both cases.

## Watching an apply

An apply is a job. It may install runtimes, create services and run a scaffold, so it can take a
while:

```bash
mix job list
mix job status <id>
mix job logs <id>
mix job wait <id>
```

`mix job logs` is where the scaffold command's own output goes — that is the one thing an apply runs
that prints anything of its own. The lines live for as long as the daemon keeps the job, so it is
what to read while one runs rather than a record to come back to next week.

If the apply needs an administrator — a new domain that needs routing, say — it asks once at the
end. `--grant` spends that prompt without asking first.
