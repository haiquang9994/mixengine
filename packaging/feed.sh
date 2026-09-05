#!/usr/bin/env bash
# Write `latest.json` — what an installed MixEngine reads to find out that a newer one exists.
#
# Roadmap task T88, the design's D13.
# Design: docs/superpowers/specs/2026-09-04-t88-self-update-design.md
#
# **Run in the `release` job, before `sign.sh`, and nowhere else.** The feed lists the payload
# archives of *every* leg, and no build leg can see the other four; and it is signed by being in the
# distribution directory when the signing step runs, which is what puts `latest.json.minisig` beside
# it under the name `mixengine_core::index::Client` appends.
#
# **The notes come from `git` and not from GitHub.** The release job's order is: gather the legs,
# sign, create the draft with `--generate-notes`, upload. So the notes GitHub generates do not exist
# until after the signing is over, and a document signed before them cannot contain them. Re-signing
# afterwards would put the private key on the machine of whoever edits the draft, which is the one
# thing T86 arranged not to need. What this writes instead is the tag's own commit subjects, and a
# `notes_url` pointing at the page somebody may have edited afterwards.

source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

mix_require python3

dist="$MIX_OUT/dist"
tag=""
repo="mixnz/mixengine"
version=""

while [ $# -gt 0 ]; do
  case "$1" in
    --dist)
      dist="$2"
      shift 2
      ;;
    --tag)
      tag="$2"
      shift 2
      ;;
    --repo)
      repo="$2"
      shift 2
      ;;
    --version)
      version="$2"
      shift 2
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 64
      ;;
  esac
done

[ -n "$version" ] || version="$(mix_version)"
[ -n "$tag" ] || tag="v$version"

# The strict spelling `mixengine_core::index::format::Timestamp` parses, and the only one it does.
now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

# **The tag's own commit subjects.** An empty answer is the first release rather than an error, and
# a repository with no history at all — a fixture — is the same case.
previous="$(git -C "$MIX_ROOT" describe --tags --abbrev=0 "$tag^" 2>/dev/null || true)"
if [ -n "$previous" ]; then
  notes="$(git -C "$MIX_ROOT" log --format='%s' "$previous..$tag" 2>/dev/null || true)"
else
  notes="$(git -C "$MIX_ROOT" log --format='%s' -20 "$tag" 2>/dev/null || true)"
fi
[ -n "$notes" ] || notes="See the release page for what changed."

# Every payload archive, and never an installer. Matched by name rather than by extension: a `.zip`
# is the Windows payload and an `.rpm` is not a payload at all, and the difference is the shape of
# the name the build scripts give them.
shopt -s nullglob
payloads=()
for file in "$dist/mixengine-$version-windows-"*.zip \
  "$dist/mixengine-$version-linux-"*.tar.gz \
  "$dist/mixengine-$version-macos-"*.tar.gz; do
  [ -f "$file" ] && payloads+=("$file")
done

if [ ${#payloads[@]} -eq 0 ]; then
  echo "no update payloads in $dist for $version" >&2
  exit 1
fi

# One row per (os, arch). macOS is universal — one archive, two architectures — so its file produces
# two rows pointing at the same URL, which is what keeps `Arch` a closed enum of two variants and a
# client's lookup a match on the pair it already has (the design, D6).
rows=""
for file in "${payloads[@]}"; do
  name="$(basename "$file")"
  size="$(wc -c <"$file" | tr -d ' ')"

  if [ -f "$file.sha256" ]; then
    sha="$(cut -d' ' -f1 <"$file.sha256")"
  else
    sha="$(sha256sum "$file" | cut -d' ' -f1)"
  fi

  # **Read out of the archive rather than assumed**, on `build.sh`'s own rule: an empty archive is a
  # perfectly valid archive, and this is the step that would notice. `provides` maps each executable
  # name to its path inside the payload, which is exactly what `core::install` reads.
  case "$name" in
    *.zip) entries="$(unzip -Z1 "$file")" ;;
    *.tar.gz) entries="$(tar -tzf "$file")" ;;
    *)
      echo "$name is not an archive shape this script can open" >&2
      exit 1
      ;;
  esac

  provides=""
  for entry in $entries; do
    case "$entry" in
      mixengine/*/* | */) continue ;;
      mixengine/*) ;;
      *) continue ;;
    esac

    # **The key is the executable's name without its extension; the value keeps it.** That is what
    # `index::format::Artifact::provides` documents (`{"php": "php.exe"}`), what
    # `updates::apply::binary_name` assumes when it appends `EXE_SUFFIX` itself, and what
    # `updates::apply::stage` looks the smoke-test executable up by. Written with the `.exe` on the
    # key, a Windows payload offered `mixengined.exe` while every reader asked for `mixengined`, so
    # `mix self-update` refused its own release with `MissingFromArtifact` — T85c, D8.
    # `packaging/feed-check.sh` is what notices, and it reproduced exactly that.
    binary="${entry#mixengine/}"
    binary="${binary%.exe}"
    provides="$provides$binary=$entry"$'\n'
  done

  if [ -z "$provides" ]; then
    echo "$name holds no binaries under mixengine/" >&2
    exit 1
  fi

  case "$name" in
    *-windows-x86_64.zip) pairs="windows x86_64" ;;
    *-windows-aarch64.zip) pairs="windows aarch64" ;;
    *-linux-x86_64.tar.gz) pairs="linux x86_64" ;;
    *-linux-aarch64.tar.gz) pairs="linux aarch64" ;;
    *-macos-universal.tar.gz) pairs="macos x86_64
macos aarch64" ;;
    *)
      echo "$name is not a payload name this script recognises" >&2
      exit 1
      ;;
  esac

  while read -r os arch; do
    [ -n "$os" ] || continue
    rows="$rows$os $arch https://github.com/$repo/releases/download/$tag/$name $sha $size"$'\n'
    rows="$rows--provides"$'\n'"$provides--end"$'\n'
  done <<<"$pairs"
done

# The privileged helper of each leg, published as its own asset — roadmap task T88a. `mix
# self-update` never replaces `mixengine-elevate`, so a release cannot deliver it inside a payload;
# what a machine fetches instead is this file and the `.minisig` `sign.sh` puts beside it, and this
# is where the feed says where they are.
#
# macOS publishes one universal helper listed under both architecture rows, exactly as its payload
# archive is — the T88 design's D6, one artifact along.
helpers=""
for file in "$dist/mixengine-elevate-$version-"*; do
  case "$file" in
    *.sha256 | *.minisig) continue ;;
  esac
  [ -f "$file" ] || continue

  name="$(basename "$file")"
  size="$(wc -c <"$file" | tr -d ' ')"
  rest="${name#mixengine-elevate-"$version"-}"
  rest="${rest%.exe}"
  helper_os="${rest%%-*}"
  helper_arch="${rest#*-}"
  url="https://github.com/$repo/releases/download/$tag/$name"

  case "$helper_os-$helper_arch" in
    macos-universal)
      helpers="$helpers"$'\n'"macos x86_64 $url $size"
      helpers="$helpers"$'\n'"macos aarch64 $url $size"
      ;;
    windows-* | linux-* | macos-*)
      helpers="$helpers"$'\n'"$helper_os $helper_arch $url $size"
      ;;
    *)
      echo "$name is not a helper name this script recognises" >&2
      exit 1
      ;;
  esac
done

if [ -z "$helpers" ]; then
  echo "no privileged helpers in $dist for $version" >&2
  exit 1
fi

# **Written by `python3` and not by `printf`**, because `notes` carries commit subjects and those
# contain quotes, backslashes and newlines. `jq` is deliberately not reached for: `common.sh` already
# records that it is not on a Git Bash install, and a release has to be buildable by hand on the
# machine that cut it.
export MIX_FEED_ROWS="$rows"
export MIX_FEED_HELPERS="$helpers"
export MIX_FEED_NOW="$now"
export MIX_FEED_VERSION="$version"
export MIX_FEED_NOTES="$notes"
export MIX_FEED_NOTES_URL="https://github.com/$repo/releases/tag/$tag"

python3 - >"$dist/latest.json" <<'PY'
import json
import os

rows = []
lines = os.environ["MIX_FEED_ROWS"].splitlines()
index = 0
while index < len(lines):
    line = lines[index]
    if not line.strip():
        index += 1
        continue

    os_name, arch, url, sha256, size = line.split(" ")
    index += 1
    assert lines[index] == "--provides", lines[index]
    index += 1

    provides = {}
    while lines[index] != "--end":
        name, path = lines[index].split("=", 1)
        provides[name] = path
        index += 1
    index += 1

    rows.append(
        {
            "os": os_name,
            "arch": arch,
            "url": url,
            "sha256": sha256,
            "size": int(size),
            "provides": provides,
        }
    )

helpers = []
for line in os.environ["MIX_FEED_HELPERS"].splitlines():
    if not line.strip():
        continue

    os_name, arch, url, size = line.split(" ")
    helpers.append({"os": os_name, "arch": arch, "url": url, "size": int(size)})

document = {
    "schema": 1,
    "generated_at": os.environ["MIX_FEED_NOW"],
    "version": os.environ["MIX_FEED_VERSION"],
    "published_at": os.environ["MIX_FEED_NOW"],
    "notes": os.environ["MIX_FEED_NOTES"].strip(),
    "notes_url": os.environ["MIX_FEED_NOTES_URL"],
    "artifacts": rows,
    "helpers": helpers,
}

print(json.dumps(document, indent=2, sort_keys=True))
PY

# **Read back what was written.** A feed that is not JSON, or that lists nothing, is a release whose
# updater is broken in a way nobody would notice until the release after it.
python3 - "$dist/latest.json" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    document = json.load(handle)

if not document["artifacts"]:
    raise SystemExit("the feed lists no artifacts")

# T88a. A release whose helper rows are missing is one where `mix elevation upgrade` answers
# "no privileged helper for this machine" for ever, and nothing else would notice.
if not document["helpers"]:
    raise SystemExit("the feed lists no privileged helpers")

print(
    f"latest.json: {document['version']}, {len(document['artifacts'])} artifact(s), "
    f"{len(document['helpers'])} helper(s)"
)
PY
