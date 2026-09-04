#!/usr/bin/env bash
# macOS: one universal `.pkg`.
#
# **A `.pkg` and not the `.dmg` the roadmap first asked for** — the T85 design, D8. A disk image is a
# carrier for something you drag out of it, and the thing that used to be dragged was an application
# bundle ADR 0011 deleted; what is left to ship here is three command-line binaries. A `.pkg` also
# runs as root, which is what lets it place the privileged helper at install time instead of leaving
# it to the first elevation prompt.

source "$(dirname "${BASH_SOURCE[0]}")/../common.sh"

mix_require lipo pkgbuild pkgutil

version="$(mix_version)"
dist="$MIX_OUT/dist"
mkdir -p "$dist"

# Both slices, then one binary per name. Apple's toolchain cross-compiles the other architecture
# with no extra sysroot, which is why macOS is universal here while Windows and Linux ship the host
# architecture alone — that second one is roadmap task T85a.
rustup target add x86_64-apple-darwin aarch64-apple-darwin
intel="$(bash "$MIX_ROOT/packaging/stage.sh" --target x86_64-apple-darwin | tail -1)"
arm="$(bash "$MIX_ROOT/packaging/stage.sh" --target aarch64-apple-darwin | tail -1)"

root="$MIX_OUT/pkgroot"
rm -rf "$root"
mkdir -p "$root/usr/local/bin" "$root/Library/PrivilegedHelperTools"

lipo -create "$intel/mix" "$arm/mix" -output "$root/usr/local/bin/mix"
lipo -create "$intel/mixengined" "$arm/mixengined" -output "$root/usr/local/bin/mixengined"

# The one file that goes somewhere only root can write, at exactly the path
# `mixengine_platform::install::helper_path()` returns — so a machine installed from this package
# finds `HelperInstall` already done and answers `AlreadyDone`.
lipo -create "$intel/mixengine-elevate" "$arm/mixengine-elevate" \
  -output "$root/Library/PrivilegedHelperTools/dev.mixengine.elevate"

chmod 755 \
  "$root/usr/local/bin/mix" \
  "$root/usr/local/bin/mixengined" \
  "$root/Library/PrivilegedHelperTools/dev.mixengine.elevate"

name="mixengine-$version-macos-universal.pkg"
rm -f "$dist/$name"

# `--ownership recommended`: the payload is installed as `root:wheel` whatever the account that
# built it happened to be, which is the whole reason the helper can be shipped in here at all.
pkgbuild \
  --root "$root" \
  --identifier dev.mixengine.cli \
  --version "$version" \
  --ownership recommended \
  --install-location / \
  "$dist/$name"

# **Open what was just made and check the three binaries are in it** — the T85 design, D11.
files="$(pkgutil --payload-files "$dist/$name")"
for expected in \
  ./usr/local/bin/mix \
  ./usr/local/bin/mixengined \
  ./Library/PrivilegedHelperTools/dev.mixengine.elevate; do
  printf '%s\n' "$files" | grep -qx "$expected" || {
    echo "$expected is not in the package" >&2
    exit 1
  }
done

# And that "universal" is true rather than asserted by the file name.
for binary in mix mixengined; do
  architectures="$(lipo -archs "$root/usr/local/bin/$binary")"
  for slice in x86_64 arm64; do
    printf '%s\n' "$architectures" | grep -qw "$slice" || {
      echo "$binary is missing the $slice slice: $architectures" >&2
      exit 1
    }
  done
done

mix_checksum "$dist/$name"

echo "$dist/$name"
