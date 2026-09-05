#!/usr/bin/env bash
# macOS: what an unsigned release looks like to Gatekeeper.
#
# Roadmap task T86a, design:
# docs/superpowers/specs/2026-09-04-t86a-unsigned-distribution-design.md
#
# **This measures the mark, not the dialog.** Gatekeeper's first-open gate is reached through
# `com.apple.quarantine`, written by the application that downloaded the file. So what a machine can
# answer is which files in a MixEngine install ever carry one, and what the assessment says about the
# package itself. The Finder dialog and macOS 15's System Settings → Privacy & Security → "Open
# Anyway" path need a person — release checklist item 4 in
# .claude/operations/build-and-release.md.
#
# Two rules this script is built on, both from the design:
#
#   * **Fail on our artifacts, record on the environment** (D3). A package that acquires a signature
#     nobody bought fails the job; a machine that cannot answer produces a *void reading*, printed
#     under its own heading, because a green job that measured nothing must not look like a green job
#     that measured and found nothing.
#   * **Never disarm the machine to get a reading** (D9). If assessments are off, M2 is void. There is
#     no `spctl --master-disable` anywhere here and there never will be: a reading taken on a machine
#     we disarmed is about our tampering and not about the product.

source "$(dirname "${BASH_SOURCE[0]}")/../common.sh"

mix_require spctl pkgutil xattr installer codesign sw_vers uuidgen

version="$(mix_version)"
dist="$MIX_OUT/dist"
probe_dir="$MIX_OUT/probe"
# **Not `$MIX_OUT/dist`** — the `release` job signs and publishes everything it finds in there, so a
# report written beside the artifacts would be signed, uploaded and listed as something to download.
report="$probe_dir/macos.md"
work="$probe_dir/work"

rm -rf "$work"
mkdir -p "$probe_dir" "$work"

# A separator no reading's answer can contain, so a row survives being split back apart.
US=$'\x1f'
readings=()
voids=()
failed=0

record() { readings+=("$1$US$2$US$(printf '%s' "$3" | tr '\n' ' ')"); }
void() { voids+=("$1$US$2"); }
fail() {
  echo "::error::$*" >&2
  failed=1
}

pkg="$dist/mixengine-$version-macos-universal.pkg"
test -f "$pkg" || {
  echo "$pkg was not built — run packaging/macos/build.sh first" >&2
  exit 1
}

# The paths this package writes, at exactly the locations `macos/build.sh` puts them.
cli=/usr/local/bin/mix
daemon=/usr/local/bin/mixengined
shim=/usr/local/bin/mixengine-shim
helper=/Library/PrivilegedHelperTools/dev.mixengine.elevate

# **One array, walked by the occupied check, by `cleanup` and by M5.** Three separate lists of the
# same paths is what T85c was; and here the cost of one going stale is concrete — a path missing
# from `cleanup` is a file this probe leaves on the machine, and the same path missing from the
# occupied check is the next run failing to notice it and then deleting it as its own.
paths=("$cli" "$daemon" "$shim" "$helper")

receipt=dev.mixengine.cli

# ---------------------------------------------------------------------------------------------
# M0 — the controls, and they come first.
#
# The roadmap asks about macOS 15+, so a machine older than that answers a nearby question rather
# than this one. And a machine with assessments disabled cannot give a Gatekeeper verdict at all —
# `spctl --assess` there is not a reading of Gatekeeper, it is a reading of a switch. Four of the six
# measurement rounds behind T45 were void for want of exactly this kind of control.

product="$(sw_vers -productVersion)"
record M0 "macOS version" "$product"
case "$product" in
15.* | 1[6-9].* | [2-9][0-9].*) ;;
*) void M0 "this machine is macOS $product and T86a asks about 15+, so every reading below is about an older Gatekeeper" ;;
esac

assessments="$(spctl --status 2>&1 || true)"
record M0 "spctl --status" "$assessments"

# ---------------------------------------------------------------------------------------------
# M1 — the package carries no signature.

signature="$(pkgutil --check-signature "$pkg" 2>&1 || true)"
case "$signature" in
*"no signature"*)
  record M1 "pkgutil --check-signature" "no signature"
  ;;
*)
  record M1 "pkgutil --check-signature" "$signature"
  fail "the package reports a signature nobody bought; T86a's findings all assume nothing in this release is OS code signed"
  ;;
esac

# ---------------------------------------------------------------------------------------------
# M2 — Gatekeeper's verdict, in its own words.

case "$assessments" in
*disabled*)
  void M2 "assessments are disabled on this machine, so spctl's answer would be the switch's and not Gatekeeper's"
  ;;
*)
  verdict="$(spctl --assess --type install --verbose=4 "$pkg" 2>&1 || true)"
  record M2 "spctl --assess --type install" "$verdict"
  case "$verdict" in
  *rejected*) ;;
  *) fail "Gatekeeper did not reject an unsigned package: $verdict" ;;
  esac
  ;;
esac

# ---------------------------------------------------------------------------------------------
# M3 — the fixture, read back before anything is concluded from it. The value is the shape Safari
# writes: flags, a hex timestamp, the agent's name, an event id.

marked="$work/mixengine.pkg"
cp "$pkg" "$marked"
xattr -w com.apple.quarantine "0081;$(printf '%x' "$(date +%s)");Safari;$(uuidgen)" "$marked" || true

if xattr -p com.apple.quarantine "$marked" >/dev/null 2>&1; then
  record M3 "a quarantine attribute can be written and read back on this volume" "yes"
  fixture=1
else
  void M3 "the quarantine attribute did not read back, so M4 and M5 conclude nothing"
  fixture=0
fi

# ---------------------------------------------------------------------------------------------
# M4 — does `installer(8)` install a quarantined, unsigned package?
#
# **The reading with the most product in it.** If it does, then the macOS instruction for a
# command-line product is one command in the terminal the user already has open, and not the System
# Settings walk `updates.md` predicts a drop-off at. If it refuses, that predicted drop-off is real
# and the recommendation to ship macOS only once there is a Developer ID gets its evidence.
#
# This probe asserts that the reading was *taken*, not which way it went.
#
# Behind `MIX_PROBE_INSTALL` (D7), and refused outright on an occupied machine: there is no `-target`
# that isolates this, the paths are the real ones, and a probe that overwrites somebody's install and
# then deletes it is worse than a probe that does not run.

installed_here=0
cleanup() {
  if [ "$installed_here" = "1" ]; then
    sudo rm -f "${paths[@]}"
    sudo pkgutil --forget "$receipt" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

if [ "${MIX_PROBE_INSTALL:-}" != "1" ]; then
  void M4 "MIX_PROBE_INSTALL is not 1, so nothing was installed on this machine"
  void M5 "M4 did not run"
  void M6 "M4 did not run"
elif [ "$fixture" != "1" ]; then
  void M4 "M3 is void: the package would have been installed without a quarantine attribute on it"
  void M5 "M4 did not run"
  void M6 "M4 did not run"
else
  occupied=""
  for path in "${paths[@]}"; do
    test ! -e "$path" || occupied="$occupied $path"
  done
  if pkgutil --pkg-info "$receipt" >/dev/null 2>&1; then
    occupied="$occupied (a $receipt receipt)"
  fi

  if [ -n "$occupied" ]; then
    echo "this machine already has MixEngine at:$occupied" >&2
    echo "M4 installs to those exact paths and removes them afterwards, which would take a real" >&2
    echo "installation with it. Uninstall first, or run the probe somewhere else." >&2
    exit 1
  fi

  # A refusal by `sudo` is not a refusal by Gatekeeper, and recording it as one would be the
  # confident wrong answer this whole file is arranged against.
  asked=1
  if ! sudo -n true >/dev/null 2>&1; then
    asked=0
    void M4 "this shell cannot become root without a password, so installer(8) was never asked"
  else
    outcome="$(sudo installer -pkg "$marked" -target / 2>&1 && echo "INSTALLER_OK" || echo "INSTALLER_REFUSED")"
    case "$outcome" in
    *INSTALLER_OK*)
      installed_here=1
      record M4 "installer(8) on a quarantined, unsigned package" "installed"
      ;;
    *)
      record M4 "installer(8) on a quarantined, unsigned package" "refused: $outcome"
      ;;
    esac
  fi

  if [ "$installed_here" = "1" ]; then
    # M5 — nothing the package installs carries quarantine, so the first run of `mix` is not gated
    # at all. The payload is written by the install daemon, and quarantine is applied by a
    # downloader rather than by a write.
    carried=""
    for path in "${paths[@]}"; do
      if [ ! -e "$path" ]; then
        fail "the package did not write $path"
        continue
      fi
      if xattr -p com.apple.quarantine "$path" >/dev/null 2>&1; then
        carried="$carried $path"
      fi
    done
    carried="${carried# }"
    record M5 "installed files carrying the package's quarantine attribute" "${carried:-none of ${#paths[@]}}"
    test -z "$carried" ||
      fail "the package passed its quarantine attribute on to $carried — the first run of each of those is now gated, which updates.md says it is not"

    # M6 — the ad-hoc signature the linker supplies is enough to execute, which is the sentence in
    # updates.md about Apple Silicon, measured rather than repeated.
    record M6 "codesign -dv on the installed mix" "$(codesign -dv "$cli" 2>&1 | tr '\n' ' ' || true)"
    if "$cli" --version >/dev/null 2>&1; then
      record M6 "the installed mix runs" "yes — $("$cli" --version 2>&1 || true)"
    else
      record M6 "the installed mix runs" "no"
      fail "an installed mix would not execute on this machine"
    fi
  elif [ "$asked" = "1" ]; then
    void M5 "installer(8) refused, so there is nothing installed to look at"
    void M6 "installer(8) refused, so there is nothing installed to run"
  else
    void M5 "M4 did not run"
    void M6 "M4 did not run"
  fi
fi

# ---------------------------------------------------------------------------------------------
# M7 — quarantine is not a property of writing a file.
#
# The Windows probe's W5 on this platform, and the same argument: quarantine is applied by a
# downloader that asks LaunchServices to apply it, so a file this product writes for itself carries
# none however it obtained the bytes. That is the empirical verification `updates.md` asks for on the
# update path — *"Updates downloaded by the already-running daemon are generally not quarantined.
# Verify this empirically before relying on it"* — and unlike the Windows half it can be tightened
# later against T88's real downloader.

written="$work/written.bin"
printf 'MZ' >"$written"
if xattr -p com.apple.quarantine "$written" >/dev/null 2>&1; then
  record M7 "a file written by an ordinary program carries quarantine" "yes"
  fail "writing a file produced a quarantine attribute; the reading behind 'an update is never gated' no longer holds"
else
  record M7 "a file written by an ordinary program carries quarantine" "no"
fi

# ---------------------------------------------------------------------------------------------
# The report. Four fixed sections, and the same four on Windows, so a reader comparing two runs does
# not have to re-learn the layout.

machine="${RUNNER_NAME:-a developer machine}${GITHUB_RUN_ID:+ · run $GITHUB_RUN_ID}"

{
  echo "# macOS — unsigned distribution probe"
  echo
  echo "Taken on $(date -u +%Y-%m-%d) · macOS $product ($(sw_vers -buildVersion)) · $machine · universal"
  echo
  echo "Artifact: \`$(basename "$pkg")\` — MixEngine $version."
  echo
  echo "## Readings"
  echo
  echo "| # | Reading | Answer |"
  echo "| --- | --- | --- |"
  for row in "${readings[@]}"; do
    IFS="$US" read -r id question answer <<<"$row"
    echo "| $id | $question | $answer |"
  done
  echo
  echo "## Void readings"
  echo
  if [ ${#voids[@]} -eq 0 ]; then
    echo "None — every reading above was taken on this machine."
  else
    for row in "${voids[@]}"; do
      IFS="$US" read -r id why <<<"$row"
      echo "- **$id** — $why"
    done
  fi
  echo
  echo "## What this does not answer"
  echo
  echo "- **The Finder dialog**, and macOS 15's System Settings → Privacy & Security → \"Open Anyway\""
  echo "  path behind it. \`installer(8)\` and \`spctl\` are not the code path a double-click takes, and"
  echo "  no runner has anybody to look at a dialog — release checklist item 4."
  echo "- **Whether an update the daemon downloads is quarantined in the real thing.** M7 measures the"
  echo "  mechanism; the downloader that will exercise it is T88's and does not exist yet."
} >"$report"

cat "$report"
test -z "${GITHUB_STEP_SUMMARY:-}" || cat "$report" >>"$GITHUB_STEP_SUMMARY"

exit "$failed"
