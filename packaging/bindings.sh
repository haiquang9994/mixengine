#!/usr/bin/env bash
# The published TypeScript API contract: generate it, check it is current, pack it for a release.
#
# Roadmap task T56.
# Design: docs/superpowers/specs/2026-09-05-t56-the-published-api-contract-design.md
#
#   bash packaging/bindings.sh            regenerate bindings/ in place
#   bash packaging/bindings.sh --check    regenerate into a temp dir and diff; writes nothing
#   bash packaging/bindings.sh --pack     archive the committed bindings/ into dist; runs no cargo
#
# **`bindings/` is generated to its last file**, barrel and README included, which is what makes
# `--check` a plain `diff -r` with nothing to exclude and what makes a deleted type take its file
# with it. Where the files go and how a `u64` is spelled live in `.cargo/config.toml` rather than
# here — see the design's D4: a generator whose answer depends on how it was invoked is not one a CI
# job can check.

source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

mode="write"
while [ $# -gt 0 ]; do
  case "$1" in
    --check)
      mode="check"
      shift
      ;;
    --pack)
      mode="pack"
      shift
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 64
      ;;
  esac
done

committed="$MIX_ROOT/bindings"

# How many declarations a contract directory holds.
mix_contract_size() {
  find "$1" -name '*.ts' ! -name 'index.ts' | wc -l | tr -d ' '
}

# Regenerate the whole contract into $1.
#
# `TS_RS_EXPORT_DIR` is exported over the one `.cargo/config.toml` sets, which is possible because
# that entry is not `force = true`.
mix_generate_contract() {
  local out="$1"

  rm -rf "$out"
  mkdir -p "$out"

  TS_RS_EXPORT_DIR="$out" cargo test -p mixengine-proto --features ts --locked --lib

  # The barrel. The list is taken before the file is written, so it cannot list itself, and
  # `LC_ALL=C` so the order is the same on every machine rather than the same in every locale.
  local files
  files="$(cd "$out" && find . -name '*.ts' | sed 's|^\./||' | LC_ALL=C sort)"

  # **A basename twice is a barrel that re-exports one name from two files.** It cannot happen while
  # type names are unique — `crates/mixengine-proto/tests/bindings.rs` asserts that — and this is
  # what would notice if it ever did.
  local duplicates
  duplicates="$(printf '%s\n' "$files" | sed 's|.*/||' | LC_ALL=C sort | uniq -d)"
  if [ -n "$duplicates" ]; then
    echo "two files in the contract share a name: $duplicates" >&2
    exit 1
  fi

  {
    echo "// The MixEngine API contract: every request, response, event and error the daemon speaks."
    echo "//"
    echo "// Generated from \`mixengine-proto\` by \`packaging/bindings.sh\` — roadmap task T56."
    echo "// Do not edit anything in this directory; every file in it is rewritten from the Rust types."
    echo ""
    printf '%s\n' "$files" | while read -r file; do
      echo "export * from \"./${file%.ts}\";"
    done
  } >"$out/index.ts"

  cat >"$out/README.md" <<'MARKDOWN'
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
import type { DaemonStatus, DaemonEvent, Error as MixEngineError } from "@mixengine/api";
```

The wire failure type is called `Error`, after the Rust type it is generated from. Rename it on
import, as above — a bare `Error` shadows the global one for the rest of the file.

The protocol version is not in this package on purpose. A client learns it from the handshake, which
is the only end of the connection that knows it.
MARKDOWN
}

case "$mode" in
  write)
    mix_generate_contract "$committed"
    echo "wrote $(mix_contract_size "$committed") types into $committed"
    ;;

  check)
    # Into a temporary directory rather than in place, and compared with `diff` rather than with
    # `git diff --exit-code`: a red job should be a message and not also a dirty checkout, and this
    # has to answer on a machine that unpacked a tarball rather than cloned one.
    work="$(mktemp -d)"
    trap 'rm -rf "$work"' EXIT
    mix_generate_contract "$work/bindings"

    if ! diff -r "$committed" "$work/bindings"; then
      echo "" >&2
      echo "The committed contract is not what mixengine-proto generates." >&2
      echo "Run: bash packaging/bindings.sh — and commit what it writes." >&2
      exit 1
    fi
    echo "the committed contract is current: $(mix_contract_size "$committed") types"
    ;;

  pack)
    # No cargo, and the committed tree as it stands. What guarantees that tree is current is that
    # the `release` job needs the `bindings` job, not a second generation here — the design's D11.
    test -f "$committed/index.ts" || {
      echo "no contract at $committed — run: bash packaging/bindings.sh" >&2
      exit 1
    }

    version="$(mix_version)"
    work="$(mktemp -d)"
    trap 'rm -rf "$work"' EXIT

    # `package/` and not the version: `npm install <tarball>` reads a single top-level directory,
    # and that is the name every registry tarball uses.
    root="$work/package"
    mkdir -p "$root"
    cp -R "$committed"/. "$root/"
    cp "$MIX_ROOT/LICENSE-MIT" "$MIX_ROOT/LICENSE-APACHE" "$root/"

    # **The version is stamped here and is not in the committed tree.** A committed, versioned
    # manifest would make "cutting a release is a version bump and nothing else" false: the bump
    # alone would leave the `bindings` job red until somebody regenerated.
    #
    # No `main`: there is not one line of runtime code in this package, so an entry point that ran
    # would be a lie about what it is.
    cat >"$root/package.json" <<JSON
{
  "name": "@mixengine/api",
  "version": "$version",
  "description": "TypeScript types for the MixEngine daemon's JSON-RPC API, generated from mixengine-proto.",
  "license": "MIT OR Apache-2.0",
  "repository": {
    "type": "git",
    "url": "git+https://github.com/mixnz/mixengine.git"
  },
  "types": "./index.ts",
  "sideEffects": false
}
JSON

    mkdir -p "$MIX_OUT/dist"
    archive="$MIX_OUT/dist/mixengine-api-$version-typescript.tar.gz"
    rm -f "$archive"
    tar -czf "$archive" -C "$work" package
    mix_checksum "$archive"

    # **Open what was just made** — the rule every other script in this directory follows, for the
    # reason `packaging/README.md` gives: an empty archive is a perfectly valid archive, and nothing
    # else in the pipeline would notice.
    listing="$(tar -tzf "$archive")"
    for entry in package/package.json package/index.ts package/README.md \
      package/LICENSE-MIT package/LICENSE-APACHE; do
      printf '%s\n' "$listing" | grep -qx "$entry" || {
        echo "missing from the archive: $entry" >&2
        exit 1
      }
    done

    packed="$(printf '%s\n' "$listing" | grep -c '\.ts$')"
    present="$(find "$committed" -name '*.ts' | wc -l | tr -d ' ')"
    test "$packed" -eq "$present" || {
      echo "the archive holds $packed TypeScript files and $committed holds $present" >&2
      exit 1
    }

    echo "$archive: $(mix_contract_size "$committed") types, version $version"
    ;;
esac
