#!/usr/bin/env bash
# T88's D13: exercise `packaging/feed.sh` on every CI run, against a fixture distribution directory.
#
# The script it tests writes the one document an installed MixEngine reads to find out that a newer
# one exists, and the only other thing that would ever run it is a release. So it is run here
# instead, and the properties that matter are asserted: the hash and the size describe the file
# beside them, `provides` is read out of the archive rather than assumed, and a universal macOS
# archive produces two rows — one per architecture — pointing at the same URL.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

version="9.9.9"
dist="$work/dist"
mkdir -p "$dist" "$work/payload/mixengine"

for binary in mix mixengined mixengine-elevate; do
  echo "not really $binary" >"$work/payload/mixengine/$binary"
done

linux="mixengine-$version-linux-x86_64.tar.gz"
macos="mixengine-$version-macos-universal.tar.gz"
tar -czf "$dist/$linux" -C "$work/payload" mixengine
tar -czf "$dist/$macos" -C "$work/payload" mixengine

# One with a `.sha256` beside it and one without, because both happen: every packaging script writes
# one, and a hand-assembled directory may not.
(cd "$dist" && sha256sum "$linux" >"$linux.sha256")

bash "$root/packaging/feed.sh" --dist "$dist" --version "$version" --tag "v$version" \
  --repo "example/mixengine"

test -f "$dist/latest.json" || {
  echo "feed.sh wrote no latest.json" >&2
  exit 1
}

python3 - "$dist/latest.json" "$dist/$linux" "$dist/$macos" "$version" <<'PY'
import hashlib
import json
import os
import re
import sys

feed_path, linux_path, macos_path, version = sys.argv[1:5]

with open(feed_path, encoding="utf-8") as handle:
    feed = json.load(handle)

assert feed["schema"] == 1, feed["schema"]
assert feed["version"] == version, feed["version"]
assert re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z", feed["generated_at"]), feed[
    "generated_at"
]
assert re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z", feed["published_at"]), feed[
    "published_at"
]
assert feed["notes"].strip(), "the feed carries no notes at all"
assert feed["notes_url"].endswith(f"/v{version}"), feed["notes_url"]

by_pair = {(row["os"], row["arch"]): row for row in feed["artifacts"]}

# Three rows out of two files: macOS is universal and is listed under both architectures, so a
# client asks with the pair it has rather than learning what "universal" means.
assert len(feed["artifacts"]) == 3, feed["artifacts"]
assert ("linux", "x86_64") in by_pair, by_pair.keys()
assert ("macos", "x86_64") in by_pair, by_pair.keys()
assert ("macos", "aarch64") in by_pair, by_pair.keys()
assert (
    by_pair[("macos", "x86_64")]["url"] == by_pair[("macos", "aarch64")]["url"]
), "the two macOS rows must name one archive"

for pair, path in [(("linux", "x86_64"), linux_path), (("macos", "x86_64"), macos_path)]:
    row = by_pair[pair]

    with open(path, "rb") as handle:
        digest = hashlib.sha256(handle.read()).hexdigest()

    assert row["sha256"] == digest, f"{pair}: {row['sha256']} != {digest}"
    assert row["size"] == os.path.getsize(path), pair
    assert row["url"].startswith("https://github.com/example/mixengine/releases/download/"), row[
        "url"
    ]

    # Read out of the archive rather than assumed: this is the field `core::install` uses to find
    # each binary inside the payload, and a wrong one is an update that fails after the download.
    assert row["provides"] == {
        "mix": "mixengine/mix",
        "mixengined": "mixengine/mixengined",
        "mixengine-elevate": "mixengine/mixengine-elevate",
    }, row["provides"]

print(f"latest.json describes {len(feed['artifacts'])} rows over 2 archives")
PY

# **An empty directory is a failure and not an empty feed.** A release whose feed lists nothing is
# one every installed copy would read and act on by doing nothing, for ever.
empty="$work/empty"
mkdir -p "$empty"
if bash "$root/packaging/feed.sh" --dist "$empty" --version "$version" --tag "v$version" \
  --repo "example/mixengine" 2>/dev/null; then
  echo "feed.sh wrote a feed for a directory with no payloads in it" >&2
  exit 1
fi

echo "packaging/feed.sh writes what a release needs, and refuses what it cannot describe"
