#!/usr/bin/env bash
# Fetch one package `mixengine-packages` publishes, unpack it, and name it in the environment.
#
#     .github/scripts/fetch-package.sh <kind> <version> <VARIABLE>
#
# The `test` job holds seven fetch steps of its own, each about forty lines of bash inside YAML, and
# each carrying details this cannot express: nginx packs a whole tree whose `conf/` the generated
# configuration reads by absolute path, PHP has a layout of its own, Redis has no Windows-on-ARM
# build to fetch. They are left alone deliberately — a refactor of seven working steps is not what
# M3 is — and this exists so the `bench` job can ask for three archives without becoming ten copies.
#
# It writes `<VARIABLE>=<directory>` into `$GITHUB_ENV`, which is what the suites read, and prints
# the directory. A missing archive is a failure rather than a skipped measurement: a bench that
# quietly measured two services would report a number nobody could compare.
set -euo pipefail

kind=${1:?a package kind, as the index publishes it}
version=${2:?the version the index publishes}
variable=${3:?the environment variable the suite reads}

case "${RUNNER_OS:-}-${RUNNER_ARCH:-}" in
  Linux-X64)     target=linux-x86_64;    ext=tar.zst ;;
  Linux-ARM64)   target=linux-aarch64;   ext=tar.zst ;;
  macOS-ARM64)   target=macos-aarch64;   ext=tar.zst ;;
  macOS-X64)     target=macos-x86_64;    ext=tar.zst ;;
  Windows-X64)   target=windows-x86_64;  ext=zip ;;
  Windows-ARM64) target=windows-aarch64; ext=zip ;;
  *) echo "::error::this runner is ${RUNNER_OS:-unknown}-${RUNNER_ARCH:-unknown}, which no target is published for"; exit 1 ;;
esac

into="$RUNNER_TEMP/$kind"
archive="$RUNNER_TEMP/$kind.$ext"
mkdir -p "$into"

curl --fail --silent --show-error --location --retry 3 --output "$archive" \
  "https://github.com/haiquang9994/mixengine-packages/releases/download/$kind-$version/$kind-$version-$target.$ext"

# Two different `tar`s, and on Windows the one on the PATH is the wrong one. Git Bash ships GNU tar,
# which reads `D:\a\_temp\caddy.zip` as a *remote host* called `D` — and cannot read a zip even once
# told otherwise. Windows itself ships bsdtar, which reads both, so it is named outright rather than
# reached through the PATH. Elsewhere the archive is `.tar.zst`: bsdtar decompresses it by magic and
# GNU tar shells out to `zstd`, so the pipe is the fallback for a tar built without it.
if [ "${RUNNER_OS:-}" = "Windows" ]; then
  "$SYSTEMROOT/System32/tar.exe" -xf "$archive" -C "$into"
else
  tar -xf "$archive" -C "$into" || zstd -dc "$archive" | tar -xf - -C "$into"
fi

echo "$variable=$into" >> "$GITHUB_ENV"
echo "$into"
