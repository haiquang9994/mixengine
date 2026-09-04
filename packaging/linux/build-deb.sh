#!/usr/bin/env bash
# Linux: a `.deb` with no maintainer scripts in it.
#
# **Nothing runs at install time, deliberately** — the T85 design, D10. Everything MixEngine does to
# a machine it does through `mixengine-elevate`, on first run, with the user watching; a package that
# only ships files has nothing to go wrong at install and nothing to undo at removal.

source "$(dirname "${BASH_SOURCE[0]}")/../common.sh"

mix_require dpkg-deb

version="$(mix_version)"
target="$(mix_host_target)"
arch="$(mix_arch_label "$target")"
deb_arch="amd64"
[ "$arch" = "aarch64" ] && deb_arch="arm64"
stage_args=(--target "$target")
[ -n "${MIX_CONTAINER:-}" ] && stage_args+=(--container "$MIX_CONTAINER")
stage="$(bash "$MIX_ROOT/packaging/stage.sh" "${stage_args[@]}" | tail -1)"
dist="$MIX_OUT/dist"
mkdir -p "$dist"

root="$MIX_OUT/debroot"
rm -rf "$root"
mkdir -p "$root/DEBIAN" "$root/usr/bin" "$root/usr/local/libexec/mixengine"

install -m 0755 "$stage/mix" "$root/usr/bin/mix"
install -m 0755 "$stage/mixengined" "$root/usr/bin/mixengined"

# **`/usr/local` from a package is against Debian policy and is on purpose** — the T85 design, D3.
# One lookup path per system, whatever put the file there: a daemon that had to look in two places
# depending on how MixEngine arrived would have two answers to the question of which file it runs as
# root. This is the path `mixengine_platform::install::helper_path()` returns, so a machine installed
# from this package finds `HelperInstall` already done.
install -m 0755 "$stage/mixengine-elevate" \
  "$root/usr/local/libexec/mixengine/mixengine-elevate"

cat >"$root/DEBIAN/control" <<EOF
Package: mixengine
Version: $version
Section: devel
Priority: optional
Architecture: $deb_arch
Maintainer: MixEngine <noreply@mixengine.dev>
Homepage: https://github.com/mixnz/mixengine
Description: A local web development environment
 Run and switch multiple PHP, Node.js, Python and Ruby versions with a bundled
 web server, databases and caches, local domains and automatic HTTPS - without
 Docker and without hand-written configuration files.
EOF

name="mixengine_$version-1_${deb_arch}.deb"
rm -f "$dist/$name"

# `--root-owner-group`: the payload is root's whatever account built it, which is what makes the
# helper's directory root-owned on the installing machine.
dpkg-deb --build --root-owner-group "$root" "$dist/$name"

# **Open what was just made and check the three binaries are in it** — the T85 design, D11.
contents="$(dpkg-deb -c "$dist/$name")"
for expected in ./usr/bin/mix ./usr/bin/mixengined ./usr/local/libexec/mixengine/mixengine-elevate; do
  printf '%s\n' "$contents" | grep -q " $expected\$" || {
    echo "$expected is not in the package" >&2
    exit 1
  }
done

mix_checksum "$dist/$name"

echo "$dist/$name"
