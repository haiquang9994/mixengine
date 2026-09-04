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
# **Not a signature, and never presented as one** — the minisign half is roadmap task T86. What this
# is for is a person who downloaded twice and wants to know whether they got the same file.
mix_checksum() {
  local file="$1"
  (cd "$(dirname "$file")" && sha256sum "$(basename "$file")" >"$(basename "$file").sha256")
}
