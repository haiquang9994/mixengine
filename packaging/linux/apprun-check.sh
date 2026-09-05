#!/usr/bin/env bash
# Exercise `AppRun`'s cache fill against a fixture — T85c.
#
# **The case worth having a fixture for is the second run**, where the cache already holds what an
# earlier build of the same version put there: the guard used to be `mix` alone, so a machine that
# had run one would never gain a binary a later build added — and the cache is what the AppImage
# actually executes, so a binary missing from it is missing from the product however well the image
# was packed.
#
# Runs anywhere: nothing here is an AppImage, nothing needs `appimagetool`, and nothing is
# Linux-specific. So the person editing `AppRun` can check it on the machine they are editing it on
# rather than on a runner an hour later.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

image="$work/image"
mkdir -p "$image/usr/bin"
printf '0.0.0-fixture\n' >"$image/VERSION"
install -m 0755 "$here/AppRun" "$image/AppRun"

# Stand-ins for the four binaries: each says its own name, and `mix` repeats its arguments so the
# hand-over at the end of `AppRun` can be checked rather than assumed.
for binary in mixengined mixengine-shim mixengine-elevate; do
  printf '#!/usr/bin/env bash\necho %s\n' "$binary" >"$image/usr/bin/$binary"
  chmod 755 "$image/usr/bin/$binary"
done
printf '#!/usr/bin/env bash\necho "mix $*"\n' >"$image/usr/bin/mix"
chmod 755 "$image/usr/bin/mix"

export XDG_CACHE_HOME="$work/cache"
cache="$XDG_CACHE_HOME/mixengine/0.0.0-fixture"

printed="$("$image/AppRun" --version)"
test "$printed" = "mix --version" || {
  echo "AppRun did not hand its arguments to mix: $printed" >&2
  exit 1
}

for binary in mix mixengined mixengine-shim mixengine-elevate; do
  test -x "$cache/$binary" || {
    echo "$binary is not in the cache AppRun filled" >&2
    exit 1
  }
done

# The stale cache: one binary gone, the version unchanged. The old guard looked at `mix`, found it,
# and copied nothing.
rm -f "$cache/mixengine-shim"
"$image/AppRun" --version >/dev/null
test -x "$cache/mixengine-shim" || {
  echo "AppRun did not repair a cache that was missing a binary" >&2
  exit 1
}

echo "AppRun fills the cache and repairs a stale one"
