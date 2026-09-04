#!/usr/bin/env bash
# Windows: what an unsigned release looks like to SmartScreen.
#
# Roadmap task T86a, design:
# docs/superpowers/specs/2026-09-04-t86a-unsigned-distribution-design.md
#
# **This measures the mark, not the verdict.** SmartScreen's Application Reputation gate is reached
# through `ShellExecute` on a file carrying Mark-of-the-Web — the `Zone.Identifier` stream a browser
# writes. No mark, no gate, whatever the file's reputation. So the question "how often does a user
# see the warning" reduces to "which files in a MixEngine install ever carry a mark", and that is a
# property of our own artifacts rather than of a cloud service. The verdict itself needs a browser, a
# reputation lookup and a person — release checklist item 4 in
# .claude/operations/build-and-release.md.
#
# Two rules this script is built on, both from the design:
#
#   * **Fail on our artifacts, record on the environment** (D3). An installer that starts
#     propagating a mark fails the job. A machine that cannot answer a question produces a *void
#     reading*, printed under its own heading — because a green job that measured nothing must not
#     look like a green job that measured and found nothing.
#   * **Never disarm the machine to get a reading** (D9). Nothing here touches a SmartScreen policy,
#     a Defender exclusion or Code Integrity. A reading taken on a machine we disarmed is about our
#     tampering and not about the product.

source "$(dirname "${BASH_SOURCE[0]}")/../common.sh"

mix_require powershell cygpath

version="$(mix_version)"
arch="$(mix_arch_label "$(mix_host_target)")"
dist="$MIX_OUT/dist"
probe_dir="$MIX_OUT/probe"
# **Not `$MIX_OUT/dist`** — the `release` job signs and publishes everything it finds in there, so a
# report written beside the artifacts would be signed, uploaded and listed as something to download.
report="$probe_dir/windows.md"
work="$probe_dir/work"

rm -rf "$work"
mkdir -p "$probe_dir" "$work"

# A separator no reading's answer can contain, so a row survives being split back apart.
US=$'\x1f'
readings=()
voids=()
failed=0

record() { readings+=("$1$US$2$US$3"); }
void() { voids+=("$1$US$2"); }
fail() {
  echo "::error::$*" >&2
  failed=1
}

# Every PowerShell call is one question, so a failure names the reading that asked it. `tr -d '\r'`
# because the answer comes back over a pipe from a Windows program into Git Bash.
ps() { powershell -NoProfile -NonInteractive -Command "$1" | tr -d '\r'; }
win() { cygpath -w "$1"; }

streams() { ps "(Get-Item -LiteralPath '$(win "$1")' -Stream * | Select-Object -ExpandProperty Stream) -join ','"; }

marked() {
  case "$(streams "$1")" in
  *Zone.Identifier*) return 0 ;;
  *) return 1 ;;
  esac
}

# Exactly what a browser writes. An array rather than an escaped `\r\n`: the value goes through bash,
# through PowerShell's parser and into an alternate data stream, and two of those three would eat a
# backtick.
apply_mark() { ps "Set-Content -LiteralPath '$(win "$1")' -Stream Zone.Identifier -Value @('[ZoneTransfer]','ZoneId=3')"; }

setup="$dist/mixengine-$version-windows-$arch-setup.exe"
zip="$dist/mixengine-$version-windows-$arch.zip"
for artifact in "$setup" "$zip"; do
  test -f "$artifact" || {
    echo "$artifact was not built — run packaging/windows/build.sh first" >&2
    exit 1
  }
done

# ---------------------------------------------------------------------------------------------
# W1 — nothing here is Authenticode signed.
#
# **This is the mechanism behind "every release resets it".** Reputation accrues either to a
# publisher, through a signature, or to a single file, through its hash. With no publisher identity
# there is only the hash, and the hash changes with every build — so the reset across two releases is
# not a thing to measure twice, it is a consequence of this reading.
#
# PE files only. A `.zip` is not one, and `Get-AuthenticodeSignature` would answer `NotSigned` for it
# in a way that means nothing at all.

plain="$work/plain"
ps "Expand-Archive -LiteralPath '$(win "$zip")' -DestinationPath '$(win "$plain")' -Force"

for name in "${MIX_BINARIES[@]}"; do
  test -f "$plain/mixengine/$name.exe" || {
    echo "$name.exe is not in the portable zip" >&2
    exit 1
  }
done

for pe in "$setup" "$plain/mixengine"/*.exe; do
  status="$(ps "(Get-AuthenticodeSignature -LiteralPath '$(win "$pe")').Status")"
  record W1 "$(basename "$pe") — Authenticode status" "$status"
  test "$status" = "NotSigned" ||
    fail "$(basename "$pe") is $status rather than NotSigned; T86a's findings all assume nothing in this release is OS code signed"
done

# ---------------------------------------------------------------------------------------------
# W2 — the fixture, read back before anything is concluded from it.

marked_setup="$work/setup.exe"
cp "$setup" "$marked_setup"
apply_mark "$marked_setup"

if marked "$marked_setup"; then
  record W2 "a Mark-of-the-Web can be written and read back on this volume" "yes"
  fixture=1
else
  void W2 "this volume did not keep an alternate data stream, so W3 and W4 conclude nothing"
  fixture=0
fi

# ---------------------------------------------------------------------------------------------
# W3 — the installer does not pass its mark on, and W6 — its PATH edit is reversible.
#
# Behind `MIX_PROBE_INSTALL` (D7): this writes `HKCU\Environment\Path` on whatever machine it runs
# on, and a probe that edits somebody's environment because they ran a script called `probe` is a
# surprise rather than a measurement.
#
# **Run through `CreateProcess`, deliberately.** Executing the marked installer from a shell is not
# `ShellExecute`, so no SmartScreen dialog can appear and nothing can block on a runner with nobody
# in front of it. That is the point: what is under test here is propagation, and the verdict is a
# person's (release checklist item 4).
#
# `MSYS2_ARG_CONV_EXCL='*'` because Git Bash rewrites arguments that look like paths, and NSIS's
# `/S` and `/D=` are exactly what that heuristic misreads.

if [ "${MIX_PROBE_INSTALL:-}" != "1" ]; then
  void W3 "MIX_PROBE_INSTALL is not 1, so nothing was installed on this machine"
  void W6 "MIX_PROBE_INSTALL is not 1, so the installer's PATH edit was not exercised"
elif [ "$fixture" != "1" ]; then
  void W3 "W2 is void: the installer would have been run without a mark on it"
  void W6 "W3 did not run"
else
  installed="$probe_dir/install"
  rm -rf "$installed"

  read_path() { ps "(Get-ItemProperty -Path 'HKCU:\\Environment' -Name Path -ErrorAction SilentlyContinue).Path"; }

  path_before="$(read_path)"
  MSYS2_ARG_CONV_EXCL='*' "$marked_setup" /S "/D=$(win "$installed")"
  path_after_install="$(read_path)"

  carried=""
  for name in "${MIX_BINARIES[@]}"; do
    if [ ! -f "$installed/$name.exe" ]; then
      fail "the installer did not write $name.exe to $installed"
      continue
    fi
    if marked "$installed/$name.exe"; then
      carried="$carried $name.exe"
    fi
  done
  carried="${carried# }"

  if [ -n "$carried" ]; then
    record W3 "binaries the installer wrote carrying a mark" "$carried"
    fail "the installer passed its Mark-of-the-Web on to $carried — a user's first run of each of those is now judged separately, which updates.md says it is not"
  else
    record W3 "binaries the installer wrote carrying a mark" "none of ${#MIX_BINARIES[@]}"
  fi

  if [ "$path_before" = "$path_after_install" ]; then
    record W6 "the installer extended this account's PATH" "no — see AddToPath's own length guard"
  else
    record W6 "the installer extended this account's PATH" "yes"
  fi

  # **`_?=` or the assertion below is a race.** `uninstall.exe /S` copies itself into the temporary
  # directory and re-executes from there, so the parent returns while the deletion is still running.
  # `_?=<dir>` runs it in place and synchronously — and then deliberately does not delete itself,
  # which is why `uninstall.exe` is removed by hand on the line after. Both halves look like bugs.
  MSYS2_ARG_CONV_EXCL='*' "$installed/uninstall.exe" /S "_?=$(win "$installed")"

  left=""
  for name in "${MIX_BINARIES[@]}"; do
    test ! -f "$installed/$name.exe" || left="$left $name.exe"
  done
  test -z "$left" || fail "the uninstaller left$left behind"

  rm -rf "$installed"

  path_after_uninstall="$(read_path)"
  if [ "$path_before" = "$path_after_uninstall" ]; then
    record W6 "PATH after the uninstall is what it was before the install" "yes"
  else
    record W6 "PATH after the uninstall is what it was before the install" "no"
    fail "the installer's PATH edit was not reversed by the uninstaller"
  fi
fi

# ---------------------------------------------------------------------------------------------
# W4 — the portable zip passes its mark on, if Explorer opens it.
#
# Two extractions of one marked zip. `Expand-Archive` is .NET's and propagates nothing; the shell
# namespace is the code path Explorer itself uses when somebody double-clicks the file, and it marks
# every file it writes. So the zip is the worse first-run download: three judged files instead of the
# installer's one.
#
# The shell half is a *reading*, not an assertion — a runner with no interactive session may have no
# shell namespace at all, and that is a void reading rather than a pass.

if [ "$fixture" = "1" ]; then
  marked_zip="$work/portable.zip"
  cp "$zip" "$marked_zip"
  apply_mark "$marked_zip"

  expanded="$work/expanded"
  ps "Expand-Archive -LiteralPath '$(win "$marked_zip")' -DestinationPath '$(win "$expanded")' -Force"
  carried=""
  for name in "${MIX_BINARIES[@]}"; do
    marked "$expanded/mixengine/$name.exe" && carried="$carried $name.exe"
  done
  carried="${carried# }"
  record W4 "Expand-Archive passed the zip's mark on to" "${carried:-none}"
  test -z "$carried" ||
    fail "Expand-Archive propagated a mark it has never propagated; W4's finding about which download path is safer no longer holds"

  shelled="$work/shelled"
  mkdir -p "$shelled"
  # One PowerShell call: a COM object that cannot be created is the void condition, and it has to be
  # detected in the same process that would have used it. 0x14 is FOF_SILENT | FOF_NOCONFIRMATION —
  # a namespace copy is asynchronous, so the wait below is part of the operation and not a sleep
  # somebody forgot to remove.
  outcome="$(ps "
    \$ErrorActionPreference='SilentlyContinue'
    \$shell = New-Object -ComObject Shell.Application
    if (\$shell -eq \$null) { 'unavailable'; exit 0 }
    \$source = \$shell.NameSpace('$(win "$marked_zip")')
    \$into = \$shell.NameSpace('$(win "$shelled")')
    if (\$source -eq \$null -or \$into -eq \$null) { 'unavailable'; exit 0 }
    \$into.CopyHere(\$source.Items(), 0x14)
    for (\$i = 0; \$i -lt 60; \$i++) {
      if (Test-Path -LiteralPath '$(win "$shelled")\\mixengine\\mix.exe') { break }
      Start-Sleep -Milliseconds 500
    }
    if (Test-Path -LiteralPath '$(win "$shelled")\\mixengine\\mix.exe') { 'extracted' } else { 'unavailable' }
  " || true)"

  if [ "$outcome" = "extracted" ]; then
    carried=""
    for name in "${MIX_BINARIES[@]}"; do
      marked "$shelled/mixengine/$name.exe" && carried="$carried $name.exe"
    done
    carried="${carried# }"
    record W4 "Explorer's own extraction passed the zip's mark on to" "${carried:-none of ${#MIX_BINARIES[@]}}"
  else
    void W4 "this machine has no shell namespace to extract with, so Explorer's half was not measured here"
  fi
else
  void W4 "W2 is void: the zip would have been extracted without a mark on it"
fi

# ---------------------------------------------------------------------------------------------
# W5 — a mark is not a property of writing a file.
#
# **No network, deliberately.** What is under test is not one HTTP client: it is that Mark-of-the-Web
# is applied by an application that chooses to call the Attachment Manager, so a downloader that does
# not call it leaves an unmarked file however it obtained the bytes. That is the reading the *updater*
# half of T86a needs, and it holds for the `mix self-update` T88 has not written yet.

written="$work/written.bin"
ps "[System.IO.File]::WriteAllBytes('$(win "$written")', [byte[]](0x4d,0x5a,0x90))"
if marked "$written"; then
  record W5 "a file written by an ordinary program carries a mark" "yes"
  fail "writing a file produced a Mark-of-the-Web; the reading behind 'an update is never judged' no longer holds"
else
  record W5 "a file written by an ordinary program carries a mark" "no"
fi

# ---------------------------------------------------------------------------------------------
# The report. Four fixed sections, and the same four on macOS, so a reader comparing two runs does
# not have to re-learn the layout.

os_version="$(ps '(Get-CimInstance Win32_OperatingSystem).Caption + " " + (Get-CimInstance Win32_OperatingSystem).Version')"
machine="${RUNNER_NAME:-a developer machine}${GITHUB_RUN_ID:+ · run $GITHUB_RUN_ID}"

{
  echo "# Windows — unsigned distribution probe"
  echo
  echo "Taken on $(date -u +%Y-%m-%d) · $os_version · $machine · $arch"
  echo
  echo "Artifacts: \`$(basename "$setup")\`, \`$(basename "$zip")\` — MixEngine $version."
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
  echo "- **SmartScreen's own verdict.** It needs a browser download of a published release, a cloud"
  echo "  reputation lookup and somebody to look at the dialog — release checklist item 4. And whether"
  echo "  it returns on the release after this one is inherently a two-release reading."
  echo "- **Smart App Control**, which is a different mechanism with a harsher answer and is neither"
  echo "  enforcing on a runner nor this task's: T41a and T94."
} >"$report"

cat "$report"
test -z "${GITHUB_STEP_SUMMARY:-}" || cat "$report" >>"$GITHUB_STEP_SUMMARY"

exit "$failed"
