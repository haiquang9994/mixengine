# 0014. An extension is not an API client, and gets no token

**Status**: Accepted
**Date**: 2026-09-02
**Replaces a promise** in [../architecture/security-model.md](../architecture/security-model.md),
whose "Client authentication" section carried a bullet marked *"Not built — arrives with T80"*.
T80 arrived and did not build it. This says why, and what stands in its place.

## Context

[../features/extensions.md](../features/extensions.md) gives an `extension.toml` a `[permissions]`
table:

```toml
[permissions]
services = ["read"]        # what the extension may call on the daemon API
network = "loopback"       # loopback | lan
filesystem = ["own-data"]  # own-data | project-roots:read
```

and says of it: *"`permissions` is enforced by the daemon: the extension's scoped token grants
exactly these."* The roadmap line for T80 repeats it, and the security model reserves a bullet for
it. Three documents, one control, and nothing built.

Building it means asking what the token would defend against. **An extension runs as the user's own
account.** It is a process this daemon spawns, with this account's rights, on a machine the security
model describes as *a developer tool on a trusted single-user machine*.

The endpoint's access control is that account. The socket is owner-only; both ends name an account
and check the one at the other end. So an extension holding a scoped token can put it down, open its
own connection to the same endpoint, and reach everything `mix` can reach — unauthenticated, because
being the user is the authentication. A token in an extension's environment is also readable by
anything else running as that user, which is the same set of things.

Making it a boundary means requiring a token on **every** connection, `mix` included. That is a
second access-control story, and this repository has already refused one on the same grounds: the
optional TCP listener requiring `Authorization: Bearer` was left out of T8 as *"a second transport
and a second access-control story for a case nobody has yet"*.

And there is no case. Not one extension in the plan calls the daemon API: Mailpit is an SMTP server
and a web UI, phpMyAdmin and Adminer talk to a database, and MixDB is handed a connection by `mix`
rather than asking for one. T82's and T83's extensions want a supervised process, a generated site
and a URL scheme — none of them wants a method.

## Decision

**There is no scoped token, and `permissions.services` is a declaration rather than a boundary.**

- Nothing is minted, nothing is stored, and nothing checks it. `ApiAccess::{Read, Write}` is parsed
  and rendered, and that is the whole of its implementation.
- It is a **consent surface**: a person is shown what an extension says it would do before they
  install it. That is the shape T78a already built for `[scaffold]`, where the exact command is
  shown and agreed to before it runs.
- Every surface that displays it says so in words. `mix extension inspect` prints *"a declaration
  shown to you, not a permission MixEngine enforces"*; the doc comment on `ApiAccess` says the same
  thing to whoever reads the type next.

**The other two permissions are enforced, and structurally rather than by a check.**

- `network` — the manifest may not write an address at all. `{listen}` is a placeholder, and it
  renders from `permissions.network` and from nothing else: `127.0.0.1` for `loopback`, `0.0.0.0`
  for `lan`. A host written out anywhere in the file, `127.0.0.1` included, is refused at parse. So
  a `loopback` extension has no way to spell any other address, and there is no rule for a future
  feature to forget to consult.
- `filesystem` — every path in a manifest must grow from `{install_dir}` or `{data_dir}`, and may
  not climb out with `..`. `own-data` is not a flag beside that; it *is* the placeholder vocabulary.
  `project-roots:read` unlocks nothing today and says so where it is declared.

## Consequences

- The security model's bullet becomes a decision instead of a promise. That document opens its
  "Client authentication" section by saying a control described but not built is how a later reader
  concludes the control exists; this is that sentence applied to one of its own bullets.
- **An extension that genuinely needs to call the API is not an extension.** Whatever it is wants a
  client's standing, and would arrive through the same door `mix` and an out-of-repo graphical
  client use — which is the door authentication would have to be built on, for all of them at once.
- A person is told less than the feature document implied and more than they were told before: the
  two permissions that hold are described as holding, and the one that does not is described as a
  disclosure, rather than all three reading alike.
- Reopening this needs a case: an extension somebody wants that has to ask the daemon something. On
  that day the question is not "how do we mint a token" but "does every connection carry one",
  because a token only some connections carry is not an access control.

## Alternatives considered

**Build the token as described.** Refused above: it defends against nothing while reading, in three
documents and one manifest, as though it defends against something.

**Drop `permissions.services` from the manifest entirely.** Tidier, and it closes the question
before anyone has asked it. Refused because the declaration is worth something on its own: what an
extension says it would do is worth showing to whoever is about to install it, and removing the
field would have to be argued about a use nobody has yet described. It is kept as what it is, and
labelled.

**Enforce it in the daemon by inspecting the caller.** The daemon cannot tell an extension's
connection from `mix`'s: both are this account, over the same endpoint, and a process identifier is
not a claim about which binary is running. Anything built on that would be a control that a rename
defeats.
