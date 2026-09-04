#!/usr/bin/env bash
# Linux: an AppImage that installs nothing.
#
# See `AppRun` beside this for the one thing it does before running `mix`, and why.

source "$(dirname "${BASH_SOURCE[0]}")/../common.sh"

# `desktop-file-validate` is `desktop-file-utils`, and appimagetool refuses to start without it —
# measured, not assumed: it exits with "desktop-file-validate command is missing" before it looks at
# the AppDir at all. Named here so a machine without it says which package to install rather than
# leaving that to a tool this script downloaded.
mix_require curl desktop-file-validate

version="$(mix_version)"
stage="$(bash "$MIX_ROOT/packaging/stage.sh" | tail -1)"
dist="$MIX_OUT/dist"
mkdir -p "$dist"

here="$MIX_ROOT/packaging/linux"
appdir="$MIX_OUT/AppDir"
rm -rf "$appdir"
mkdir -p "$appdir/usr/bin"

for binary in "${MIX_BINARIES[@]}"; do
  install -m 0755 "$stage/$binary" "$appdir/usr/bin/$binary"
done

install -m 0755 "$here/AppRun" "$appdir/AppRun"
install -m 0644 "$here/mixengine.desktop" "$appdir/mixengine.desktop"

# A 16x16 placeholder, because appimagetool refuses an AppDir with no icon and this product has no
# artwork yet. Committed rather than generated, and named here rather than smuggled: replacing it is
# a design task and not a packaging one.
install -m 0644 "$here/mixengine.png" "$appdir/mixengine.png"

printf '%s\n' "$version" >"$appdir/VERSION"

# Pinned to a release rather than to `continuous`, so a tool that changes its output changes it when
# this line changes and not on somebody else's Tuesday.
tool="$MIX_OUT/appimagetool"
if [ ! -x "$tool" ]; then
  curl --fail --silent --show-error --location --retry 3 --output "$tool" \
    "https://github.com/AppImage/appimagetool/releases/download/1.9.0/appimagetool-x86_64.AppImage"
  chmod 755 "$tool"
fi

name="mixengine-$version-linux-x86_64.AppImage"
rm -f "$dist/$name"

# `APPIMAGE_EXTRACT_AND_RUN=1`: the runner has no FUSE, and an AppImage that cannot mount itself
# cannot run the tool inside it. The environment variable rather than the `--appimage-extract-and-run`
# argument, because every type-2 runtime honours the variable and only newer ones parse the flag —
# and a flag the runtime does not recognise is one it passes through to the program inside.
ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 "$tool" "$appdir" "$dist/$name"
chmod 755 "$dist/$name"

# **Run what was just made rather than reading its table of contents** — the T85 design, D11, and
# the one artifact where that is possible. `mix --version` is the cheapest end-to-end proof that the
# AppRun, the extraction and the binary all work; the printed version is what says the binary inside
# is this build.
printed="$(APPIMAGE_EXTRACT_AND_RUN=1 "$dist/$name" --version)"
case "$printed" in
  *"$version"*) ;;
  *)
    echo "the AppImage printed '$printed', which does not name version $version" >&2
    exit 1
    ;;
esac

# And the helper really is in there, since nothing above would have run it.
test -x "$appdir/usr/bin/mixengine-elevate" || {
  echo "mixengine-elevate is not in the AppDir" >&2
  exit 1
}

mix_checksum "$dist/$name"

echo "$dist/$name"
