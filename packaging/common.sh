#!/usr/bin/env bash
# Shared by the per-OS packaging scripts. Sourced, never run.
#
# Bash on all three systems, because CI already runs `shell: bash` on the Windows runner and one
# language for six artifacts is one language to get right. See the T85 design, D9 and D10.

set -euo pipefail

# The repository root, however this was invoked.
MIX_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export MIX_ROOT

# Everything is built and staged under `target/`, which every machine already ignores.
MIX_OUT="$MIX_ROOT/target/packaging"
export MIX_OUT

# The three binaries a release is made of, in the order a reader wants them.
MIX_BINARIES=(mix mixengined mixengine-elevate)
export MIX_BINARIES

# macOS ships `shasum -a 256` and no `sha256sum`. Defined once here, so the three scripts do not
# each discover it.
if ! command -v sha256sum >/dev/null 2>&1; then
  sha256sum() { shasum -a 256 "$@"; }
fi

# The workspace version, read from the one place it is written.
#
# `sed` over the `[workspace.package]` block rather than `cargo metadata` piped through `jq`: jq is
# not on a Git Bash install, and a release has to be buildable by hand on the machine that cut it.
mix_version() {
  sed -n '/^\[workspace\.package\]/,/^\[/p' "$MIX_ROOT/Cargo.toml" \
    | sed -n 's/^version = "\(.*\)"$/\1/p' \
    | head -1
}

# The single source of truth for "which architecture is this leg" — T85a design, D4. An explicit
# override takes priority, because the two Linux legs that build inside a container have no `rustc`
# on the runner itself to ask; everywhere else this asks the toolchain that is about to build rather
# than `uname -m`, which an emulated shell can misreport.
mix_host_target() {
  if [ -n "${MIX_TARGET:-}" ]; then
    echo "$MIX_TARGET"
  else
    rustc -vV | sed -n 's/^host: //p'
  fi
}

# The per-package-format spelling of a target triple's architecture. `.deb`'s `Architecture:` field
# wants `amd64`/`arm64` and is translated in `build-deb.sh` alone; every other artifact name in this
# product says `x86_64`/`aarch64`, which is what this returns.
mix_arch_label() {
  case "$1" in
    x86_64-*) echo x86_64 ;;
    aarch64-*) echo aarch64 ;;
    *)
      echo "unrecognised target: $1" >&2
      return 1
      ;;
  esac
}

# Runs `cmd` inside `container`, with this repository bind-mounted at `/work` and a rustup toolchain
# matching `rust-toolchain.toml` installed first — T85a, D2/D3. Used by both Linux legs of the `build`
# job, which link against an older glibc than the runner ships by compiling inside a manylinux_2_28
# image rather than on the runner directly. The container always matches the runner's own
# architecture, so this is never cross-compilation, only an older sysroot.
mix_in_container() {
  local container="$1"
  local cmd="$2"
  local channel uid gid
  channel="$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$MIX_ROOT/rust-toolchain.toml")"
  # The container runs as root — it needs to for `dnf install` — and `chown`s `target/` back to the
  # invoking user as its last act, so nothing this leaves behind is root-owned on the host. The
  # packages below are the ones `mixengine-packages`' own AlmaLinux 8 recipe already installs for its
  # PHP/Ruby builds, named here rather than reached for blind.
  uid="$(id -u)"
  gid="$(id -g)"
  docker run --rm \
    -v "$MIX_ROOT:/work" -w /work \
    "$container" \
    bash -c "
      set -euo pipefail
      dnf install -y dbus-devel openssl-devel perl-core make gcc gcc-c++
      curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain '$channel'
      source \"\$HOME/.cargo/env\"
      $cmd
      chown -R $uid:$gid /work/target
    "
}

# Refuse early and by name, rather than half-building an artifact and failing on the tool that wraps
# it. A missing packaging tool is a machine that was not set up, and the message should say which.
mix_require() {
  local missing=()
  local tool
  for tool in "$@"; do
    command -v "$tool" >/dev/null 2>&1 || missing+=("$tool")
  done

  if [ ${#missing[@]} -ne 0 ]; then
    echo "missing tools: ${missing[*]}" >&2
    return 1
  fi
}

# A checksum beside the artifact.
#
# **Not a signature, and never presented as one** — the minisign half is `sign.sh`, which signs the
# artifact itself and deliberately skips these files. What this is for is a person who downloaded
# twice and wants to know whether they got the same file.
mix_checksum() {
  local file="$1"
  (cd "$(dirname "$file")" && sha256sum "$(basename "$file")" >"$(basename "$file").sha256")
}
