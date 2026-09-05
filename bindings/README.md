# The MixEngine API contract

TypeScript types for every request, response, event and error `mixengined` speaks over its local
JSON-RPC transport. Generated from the `mixengine-proto` crate with
[ts-rs](https://github.com/Aleph-Alpha/ts-rs); **do not edit** — every file here is rewritten from
the Rust types by `packaging/bindings.sh`, and CI fails when the two disagree.

MixEngine ships no graphical client. One lives in its own repository and reaches the daemon through
this contract, which is why it is committed here and published as an archive on every release.

## What it says, and what it does not

These types describe **what the daemon writes**, and the strict form of what it reads. A few
requests are deliberately more forgiving than that — a duration may arrive as `"10s"` as well as
`10000`, an environment value as a bare string as well as its tagged form — and the contract does
not describe those alternatives. Send the shape below and it is always accepted.

Integers are `number`, never `bigint`: these values arrive through `JSON.parse`, which produces one
and not the other.

## Using it

There is no runtime code here at all, so nothing to build and nothing to import at run time:

```ts
import type { DaemonStatus, DaemonEvent, Error } from "@mixengine/api";
```

The protocol version is not in this package on purpose. A client learns it from the handshake, which
is the only end of the connection that knows it.
