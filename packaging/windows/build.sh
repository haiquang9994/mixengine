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
stage="$(bash "$MIX_ROOT/packaging/stage.sh" | tail -1)"
dist="$MIX_OUT/dist"
mkdir -p "$dist"

zip_name="mixengine-$version-windows-x86_64.zip"
setup_name="mixengine-$version-windows-x86_64-setup.exe"

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

# **Open what was just made and check the three binaries are in it** — the T85 design, D11. An empty
# archive is a perfectly valid archive, and this is the only step that would notice.
for name in mix.exe mixengined.exe mixengine-elevate.exe; do
  unzip -l "$dist/$zip_name" | grep -q "$name" || {
    echo "$name is not in the zip" >&2
    exit 1
  }
  7z l "$dist/$setup_name" | grep -q "$name" || {
    echo "$name is not in the installer" >&2
    exit 1
  }
done

mix_checksum "$dist/$zip_name"
mix_checksum "$dist/$setup_name"

echo "$dist/$zip_name"
echo "$dist/$setup_name"
