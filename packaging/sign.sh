#!/usr/bin/env bash
# Sign every artifact in a distribution directory, and prove the signatures are ones MixEngine will
# accept — roadmap task T86.
#
# Design: docs/superpowers/specs/2026-09-04-t86-updater-signing-design.md
#
# **Never add `set -x` to this file.** The password reaches minisign on stdin and the secret key is
# written to disk for the length of the run; a trace would put both in a log.

source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

mix_require minisign

dist="$MIX_OUT/dist"
key="$HOME/.config/mixengine/updates.key"
pubkey=""
version=""

while [ $# -gt 0 ]; do
  case "$1" in
    --dist)
      dist="$2"
      shift 2
      ;;
    --key)
      key="$2"
      shift 2
      ;;
    --pubkey)
      pubkey="$2"
      shift 2
      ;;
    --version)
      version="$2"
      shift 2
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 64
      ;;
  esac
done

# **What the product will accept**, read out of the one file that decides it. Verifying against this
# rather than against whatever the signing key happens to be is the difference between "somebody
# signed it" and "MixEngine will take it" — and it is the only check anywhere that can catch an
# Actions secret which is not the pair of the committed public key.
#
# `--pubkey` overrides it for `.github/scripts/test-sign.sh` and for nothing else; a release never
# passes it, which is what keeps the release path tied to the compiled-in constant.
if [ -z "$pubkey" ]; then
  pinned="$(sed -n 's/^pub const PUBLIC_KEY: &str = "\(.*\)";$/\1/p' \
    "$MIX_ROOT/crates/mixengine-core/src/updates.rs")"
  if [ -z "$pinned" ]; then
    echo "no PUBLIC_KEY in crates/mixengine-core/src/updates.rs" >&2
    exit 1
  fi

  committed="$(sed -n '2p' "$MIX_ROOT/packaging/updates.pub" | tr -d '\r')"
  if [ "$pinned" != "$committed" ]; then
    echo "packaging/updates.pub is not the key this build pins" >&2
    echo "  pinned:    $pinned" >&2
    echo "  committed: $committed" >&2
    exit 1
  fi

  pubkey="$pinned"
fi

# `.sha256` is not an artifact and `.minisig` is not one either. Everything else in the directory is
# something a person downloads, whatever a later task adds.
shopt -s nullglob
artifacts=()
for file in "$dist"/*; do
  case "$file" in
    *.sha256 | *.minisig) continue ;;
  esac
  [ -f "$file" ] && artifacts+=("$file")
done

if [ ${#artifacts[@]} -eq 0 ]; then
  echo "nothing to sign in $dist" >&2
  exit 1
fi

[ -n "$version" ] || version="$(mix_version)"

# **Every signature carries a trusted comment, and one of them is read back by a program** — roadmap
# task T88a.
#
# minisign's *global* signature covers this text, which makes it the one place a fact about a signed
# file can travel without being taken on trust. `mixengine-elevate` needs exactly that: the elevated
# process installing a replacement has verified the bytes and still has no other way to learn which
# version, and which machine, they are for. `mixengine_proto::privileged::HelperStamp` is what reads
# it back, and it refuses anything that is not this grammar.
#
# One rule and not a special case: every artifact gets `<name> <version> <os> <arch>` where `<name>`
# is what the file is called. For the helper that name *is* `mixengine-elevate`, which is what makes
# the stamp parse; for everything else the fields are there for a person reading `minisign -V`.
comment_for() {
  local base rest
  base="$(basename "$1")"

  case "$base" in
    mixengine-elevate-"$version"-*)
      rest="${base#mixengine-elevate-"$version"-}"
      rest="${rest%.exe}"
      echo "mixengine-elevate $version ${rest%%-*} ${rest#*-}"
      ;;
    *) echo "mixengine $version $base" ;;
  esac
}

# The secret key spends the run in a file only this user can read, and leaves however this exits.
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
secret="$work/updates.key"
(
  umask 077
  if [ -n "${MIX_SIGN_SECRET_KEY:-}" ]; then
    printf '%s\n' "$MIX_SIGN_SECRET_KEY" >"$secret"
  else
    cp "$key" "$secret"
  fi
)

for file in "${artifacts[@]}"; do
  comment="$(comment_for "$file")"

  if [ -n "${MIX_SIGN_PASSWORD:-}" ]; then
    printf '%s\n' "$MIX_SIGN_PASSWORD" \
      | minisign -S -s "$secret" -t "$comment" -m "$file" >/dev/null
  else
    minisign -S -s "$secret" -t "$comment" -m "$file" >/dev/null
  fi

  # `-H` requires the prehashed form, which is exactly the `allow_legacy = false` both shipped
  # verifiers pass. A signature this refuses is one the product would refuse on a user's machine.
  minisign -V -H -P "$pubkey" -m "$file" -q || {
    echo "$(basename "$file"): the signature just made is not one this build would accept" >&2
    exit 1
  }
done

# **Count.** A release with one unsigned artifact in it is the failure this script exists to prevent,
# and a glob that quietly matched nothing is how it would happen.
signatures=("$dist"/*.minisig)
if [ ${#signatures[@]} -ne ${#artifacts[@]} ]; then
  echo "signed ${#artifacts[@]} artifacts but $dist holds ${#signatures[@]} signatures" >&2
  exit 1
fi

echo "signed ${#artifacts[@]} artifacts in $dist against $pubkey"
