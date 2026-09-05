#!/usr/bin/env bash
# T86's D9: exercise `packaging/sign.sh` on every CI run, with a key nobody has to protect.
#
# The script it tests is the only thing standing between a release and an unsigned artifact, and the
# only other thing that would ever run it is a release. So it is run here instead, against a
# throwaway key, and the two properties that matter are asserted: everything that is an artifact gets
# a signature, and a key that is not the one being verified against is refused.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# Password-protected on purpose. The stdin path is the part of this design a new minisign release
# could break, and an unencrypted key would not exercise it.
password="not-the-release-key"
printf '%s\n%s\n' "$password" "$password" | minisign -G -p "$work/mine.pub" -s "$work/mine.key" >/dev/null
printf '%s\n%s\n' "$password" "$password" | minisign -G -p "$work/other.pub" -s "$work/other.key" >/dev/null

mine="$(sed -n '2p' "$work/mine.pub" | tr -d '\r')"
other="$(sed -n '2p' "$work/other.pub" | tr -d '\r')"

dist="$work/dist"
mkdir -p "$dist"
echo "an installer" >"$dist/mixengine-0.0.0-linux-x86_64.deb"
echo "a portable zip" >"$dist/mixengine-0.0.0-windows-x86_64.zip"
echo "a privileged helper" >"$dist/mixengine-elevate-0.0.0-linux-x86_64"
(cd "$dist" && sha256sum mixengine-0.0.0-linux-x86_64.deb >mixengine-0.0.0-linux-x86_64.deb.sha256)

MIX_SIGN_PASSWORD="$password" bash "$root/packaging/sign.sh" \
  --dist "$dist" --key "$work/mine.key" --pubkey "$mine" --version 0.0.0

for artifact in mixengine-0.0.0-linux-x86_64.deb mixengine-0.0.0-windows-x86_64.zip \
  mixengine-elevate-0.0.0-linux-x86_64; do
  test -f "$dist/$artifact.minisig" || {
    echo "$artifact came out of sign.sh unsigned" >&2
    exit 1
  }
done

# A checksum is not an artifact. Signing one would be a second, weaker way of saying what the
# signature over the artifact already says.
test ! -f "$dist/mixengine-0.0.0-linux-x86_64.deb.sha256.minisig" || {
  echo "sign.sh signed a .sha256 file" >&2
  exit 1
}

# **The trusted comment on the helper, which is the one signature a *program* reads back** — roadmap
# task T88a. `mixengine_proto::privileged::HelperStamp` refuses anything that is not
# `mixengine-elevate <version> <os> <arch>`, and a helper signed without it would verify perfectly
# and then be refused by every machine that fetched it. Nothing else in CI would notice.
comment="$(sed -n '3p' "$dist/mixengine-elevate-0.0.0-linux-x86_64.minisig")"
expected="trusted comment: mixengine-elevate 0.0.0 linux x86_64"
test "$comment" = "$expected" || {
  echo "the helper's trusted comment is $comment" >&2
  echo "and HelperStamp::parse reads $expected" >&2
  exit 1
}

# And the comment is *covered* by the signature rather than sitting beside it — which is the whole
# reason a fact may travel in it. `minisign -V` checks the global signature over both.
minisign -V -H -P "$mine" -m "$dist/mixengine-elevate-0.0.0-linux-x86_64" -q || {
  echo "the helper's signature does not verify against the key that made it" >&2
  exit 1
}

# The release path's own check, exercised by the only thing that can exercise it without the real
# secret: a private key that is not the pair of the public key being verified against must fail
# before anything is published.
rm -f "$dist"/*.minisig
if MIX_SIGN_PASSWORD="$password" bash "$root/packaging/sign.sh" \
  --dist "$dist" --key "$work/mine.key" --pubkey "$other" >/dev/null 2>&1; then
  echo "sign.sh accepted a key that is not the one it verified against" >&2
  exit 1
fi

echo "packaging/sign.sh: signs every artifact, stamps the helper, signs no metadata, refuses the \
wrong key"
