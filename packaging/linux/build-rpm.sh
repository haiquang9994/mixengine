#!/usr/bin/env bash
# Linux: an `.rpm`, from the same three staged binaries the `.deb` uses.
#
# No maintainer scripts here either, for `build-deb.sh`'s reason.

source "$(dirname "${BASH_SOURCE[0]}")/../common.sh"

mix_require rpmbuild rpm

version="$(mix_version)"
stage="$(bash "$MIX_ROOT/packaging/stage.sh" | tail -1)"
dist="$MIX_OUT/dist"
mkdir -p "$dist"

build="$MIX_OUT/rpmbuild"
rm -rf "$build"
mkdir -p "$build/SOURCES" "$build/SPECS" "$build/RPMS" "$build/BUILD" "$build/BUILDROOT"
cp "$stage"/* "$build/SOURCES/"

sed "s/@VERSION@/$version/" "$MIX_ROOT/packaging/linux/mixengine.spec.in" \
  >"$build/SPECS/mixengine.spec"

rpmbuild --define "_topdir $build" -bb "$build/SPECS/mixengine.spec"

name="mixengine-$version-1.x86_64.rpm"
rm -f "$dist/$name"
cp "$build/RPMS/x86_64/$name" "$dist/$name"

# **Open what was just made and check the three binaries are in it** — the T85 design, D11.
contents="$(rpm -qlp "$dist/$name")"
for expected in /usr/bin/mix /usr/bin/mixengined /usr/local/libexec/mixengine/mixengine-elevate; do
  printf '%s\n' "$contents" | grep -qx "$expected" || {
    echo "$expected is not in the package" >&2
    exit 1
  }
done

mix_checksum "$dist/$name"

echo "$dist/$name"
