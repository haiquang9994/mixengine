+++
title = "Removing MixEngine"
slug = "uninstalling"
order = 13
summary = "Undo everything MixEngine wrote outside its own directory, see the list before you agree, and keep your databases if you want them."
+++

# Removing MixEngine

MixEngine writes almost everything inside one directory. The exceptions are the handful of
privileged changes it asked permission for, and taking those back is what `mix uninstall` is for.

## See the list first

```bash
mix uninstall --dry-run
```

That changes nothing and names every single thing it would remove:

- the hosts block, and the DNS or resolver rule that routes your names
- the grant to listen on port 80 and 443
- the certificate authority, from every store that trusts it
- any firewall rule left by a shared site
- the entry that starts the daemon when you log in
- the `PATH` entry
- the privileged helper, and its audit log
- and finally MixEngine's own directory

## Doing it

```bash
mix uninstall
```

You are asked to confirm, and one administrator prompt covers the privileged half. `--yes` answers
the confirmation in advance, for a script.

**The report is a measurement, not a claim.** What comes back is what MixEngine found on the machine
*afterwards*, row by row, including the rows that answered *nothing there* — a report that hid those
would leave you unable to tell "there was no resolver wiring" from "the resolver wiring was not
looked at". The command exits non-zero if anything it acted on is still present, so a script can
ask.

Expect the connection to end partway: the daemon is removing the home it serves, so it stops itself.
That is the normal ending, and MixEngine reads the final rows back off disk once it has, which is
what makes the answer *nothing is left behind* rather than *the daemon said so*.

## Keeping your data

```bash
mix uninstall --keep-home
```

This undoes everything **outside** the home directory and leaves the home where it is: your
databases in `data/`, your certificates, your projects' records. The daemon keeps running, because
there is still a home for it to serve.

It is the right command when you are handing the machine's network configuration back but are not
finished with the data yet.

## Then remove the program itself

`mix uninstall` removes what MixEngine did. Removing MixEngine is your package manager's job, and it
depends on how you installed it:

```bash
sudo dpkg -r mixengine
sudo rpm -e mixengine
sudo rm -rf /usr/local/bin/mix /usr/local/bin/mixengined /usr/local/bin/mixengine-shim
```

On Windows, use Apps & Features for the installer, or delete the folder for the portable zip. On
macOS, the third line above is what the `.pkg` placed. An AppImage is one file you delete.

## What is deliberately not automatic

The audit log the privileged helper keeps is root-owned, and so is the helper itself. `mix doctor`
reports both and removes neither: a diagnostic that deleted a root-owned audit trail would be
deleting the record of what it was diagnosing. `mix uninstall` is the command that removes them, and
it asks.
