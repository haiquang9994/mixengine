#!/usr/bin/env bash
# Build the release binaries and put the three of them in one directory.
#
# Every per-OS script starts here, so "what is in a release" is written once and not three times.
# Prints the staging directory on its last line; callers read it with `| tail -1`.
#
# `--target <triple>` is always passed by every caller — T85a, D5 — even on a native build, so no
# script is silently trusting cargo's own default. `--container <image>` additionally builds inside
# that image rather than on the runner directly, for a leg that wants an older glibc than the runner
# ships — T85a, D2/D3.

source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

target=""
container=""
while [ $# -gt 0 ]; do
  case "$1" in
    --target)
      target="$2"
      shift 2
      ;;
    --container)
      container="$2"
      shift 2
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 64
      ;;
  esac
done

if [ -n "$container" ] && [ -z "$target" ]; then
  echo "--container needs --target" >&2
  exit 64
fi

# `--locked`, so a packaging run cannot quietly resolve a dependency the tested build did not have.
if [ -n "$container" ]; then
  mix_in_container "$container" \
    "rustup target add '$target' && cargo build --release --locked --target '$target' -p mixengine-cli -p mixengine-daemon -p mixengine-elevate"
  built="$MIX_ROOT/target/$target/release"
  stage="$MIX_OUT/stage/$target"
elif [ -n "$target" ]; then
  cargo build --release --locked --target "$target" \
    -p mixengine-cli -p mixengine-daemon -p mixengine-elevate
  built="$MIX_ROOT/target/$target/release"
  stage="$MIX_OUT/stage/$target"
else
  cargo build --release --locked -p mixengine-cli -p mixengine-daemon -p mixengine-elevate
  built="$MIX_ROOT/target/release"
  stage="$MIX_OUT/stage/host"
fi

rm -rf "$stage"
mkdir -p "$stage"

suffix=""
case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*) suffix=".exe" ;;
esac

for binary in "${MIX_BINARIES[@]}"; do
  cp "$built/$binary$suffix" "$stage/$binary$suffix"
done

# **A stage missing a binary is the failure this whole job exists to notice**, and it is not one any
# wrapper below would report: a zip of two files is a perfectly good zip, and a `.deb` with no helper
# in it installs cleanly and leaves the machine one file short of being able to elevate.
for binary in "${MIX_BINARIES[@]}"; do
  test -f "$stage/$binary$suffix" || {
    echo "missing from the stage: $binary$suffix" >&2
    exit 1
  }
done

echo "$stage"
