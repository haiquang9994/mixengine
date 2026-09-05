#!/usr/bin/env bash
# Windows: a portable zip and a per-user NSIS installer.

source "$(dirname "${BASH_SOURCE[0]}")/../common.sh"

# `unzip` comes with Git Bash, which is the shell this runs in; `7z` is on the runner image and is
# the only thing that can list the inside of an NSIS installer.
mix_require unzip 7z

# **Resolved before the build and not after it**, which is what the first CI run of this script paid
# to learn: it compiled the workspace for seven minutes and then stopped at "missing tools: makensis".
# Every other tool is checked on the line above for the same reason, and this one is not on `PATH`.
makensis="${MAKENSIS:-/c/Program Files (x86)/NSIS/makensis.exe}"
if [ ! -x "$makensis" ]; then
  makensis="makensis"
  mix_require makensis
fi

version="$(mix_version)"
target="$(mix_host_target)"
arch="$(mix_arch_label "$target")"
stage="$(bash "$MIX_ROOT/packaging/stage.sh" --target "$target" | tail -1)"
dist="$MIX_OUT/dist"
mkdir -p "$dist"

zip_name="mixengine-$version-windows-$arch.zip"
setup_name="mixengine-$version-windows-$arch-setup.exe"

# The zip holds one directory, so unzipping it into Downloads does not scatter three binaries there.
# Written with `Compress-Archive` rather than with `7z`: it ships with Windows, so the portable
# artifact needs nothing installed to build.
rm -rf "$MIX_OUT/zip"
mkdir -p "$MIX_OUT/zip/mixengine"
cp "$stage"/*.exe "$MIX_OUT/zip/mixengine/"
rm -f "$dist/$zip_name"
powershell -NoProfile -NonInteractive -Command \
  "Compress-Archive -Path '$(cygpath -w "$MIX_OUT/zip/mixengine")' -DestinationPath '$(cygpath -w "$dist/$zip_name")' -Force"

"$makensis" -NOCD \
  "-DVERSION=$version" \
  "-DSTAGE=$(cygpath -w "$stage")" \
  "-DOUTFILE=$(cygpath -w "$dist/$setup_name")" \
  "$(cygpath -w "$MIX_ROOT/packaging/windows/mixengine.nsi")"

# **Open what was just made and check the binaries are in it** — the T85 design, D11. An empty
# archive is a perfectly valid archive, and this is the only step that would notice.
#
# `MIX_BINARIES` rather than a list written out here: a check that asserts three of the four names
# is a check that would not have caught T85c either.
#
# **Listed once into a variable and never piped into `grep -q`.** That pipeline kills the lister
# with a SIGPIPE the moment the match is found and — under `pipefail`, which `common.sh` sets —
# reports a perfectly good artifact as broken for holding exactly what was looked for. Measured on
# both Linux legs of run 33906595994; see the note in `packaging/linux/build-tarball.sh`.
zip_entries="$(unzip -l "$dist/$zip_name")"
setup_entries="$(7z l "$dist/$setup_name")"
for binary in "${MIX_BINARIES[@]}"; do
  grep -qF "$binary.exe" <<<"$zip_entries" || {
    echo "$binary.exe is not in the zip" >&2
    exit 1
  }
  grep -qF "$binary.exe" <<<"$setup_entries" || {
    echo "$binary.exe is not in the installer" >&2
    exit 1
  }
done

mix_checksum "$dist/$zip_name"
mix_checksum "$dist/$setup_name"

echo "$dist/$zip_name"
echo "$dist/$setup_name"
