#!/usr/bin/env bash
# `feed.sh` against a fixture distribution — T85c, D8.
#
# **What this looks at is the shape of the `provides` keys.** `index::format::Artifact` documents
# them as executable *name* to path (`{"php": "php.exe"}`), `updates::apply::binary_name` appends
# this platform's suffix itself, and `updates::apply::stage` looks the smoke-test executable up as
# `mixengined`. A Windows payload described with the `.exe` on the key satisfies none of those, and
# the only sign of it is a `mix self-update` that refuses the release it was just offered.
#
# No packaging tools, and nothing OS-specific: the fixture archives are written by `tar` and by
# `python3`'s own `zipfile`, and `feed.sh` already requires `python3`. So this runs on the machine of
# whoever is editing the script, whichever of the three that is.

set -euo pipefail

source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

version="$(mix_version)"
work="$MIX_OUT/feed-check"
rm -rf "$work"
mkdir -p "$work/dist" "$work/payload/mixengine"

for binary in "${MIX_BINARIES[@]}"; do
  printf 'not a binary\n' >"$work/payload/mixengine/$binary"
done
tar -czf "$work/dist/mixengine-$version-linux-x86_64.tar.gz" -C "$work/payload" mixengine

# The Windows payload, whose entries carry `.exe` — the whole point of the check.
export MIX_CHECK_ZIP="$work/dist/mixengine-$version-windows-x86_64.zip"
export MIX_CHECK_NAMES="${MIX_BINARIES[*]}"

python3 - <<'PY'
import os
import zipfile

with zipfile.ZipFile(os.environ["MIX_CHECK_ZIP"], "w") as archive:
    for name in os.environ["MIX_CHECK_NAMES"].split():
        archive.writestr(f"mixengine/{name}.exe", "not a binary\n")
PY

bash "$MIX_ROOT/packaging/feed.sh" --dist "$work/dist" --version "$version" --tag "v$version"

python3 - "$work/dist/latest.json" <<'PY'
import json
import os
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    document = json.load(handle)

expected = set(os.environ["MIX_CHECK_NAMES"].split())
problems = []

for artifact in document["artifacts"]:
    provides = artifact["provides"]
    names = set(provides)

    if names != expected:
        where = artifact["os"] + "/" + artifact["arch"]
        problems.append(f"{where} provides {sorted(names)}, not {sorted(expected)}")

    for name, path in provides.items():
        if not path.startswith("mixengine/"):
            problems.append(f"{name} points at {path}, which is not under mixengine/")

if problems:
    raise SystemExit("\n".join(problems))

print(f"provides: {len(document['artifacts'])} artifact(s), each naming {sorted(expected)}")
PY
