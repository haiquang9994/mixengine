#!/usr/bin/env bash
# Watch the CI run this branch asked for, and say what failed rather than that something did.
#
# CI fires by itself on `master` and on nothing else — see
# `.claude/operations/build-and-release.md`. Every other branch pushes and then requests a run, and
# this is the loop that waits for the answer: it polls `gh run view`, prints each job as it settles,
# and on a failure prints an extract of the failing steps' logs so the next thing to read is the
# error and not a URL.
#
#   scripts/watch-ci.sh                 # the run for HEAD on the current branch
#   scripts/watch-ci.sh 32617405595     # a run by id
#   scripts/watch-ci.sh --once          # say where the run is now and exit; 2 means still running
#   INTERVAL=60 scripts/watch-ci.sh     # poll less often
#   EXTRACT_LINES=500 scripts/watch-ci.sh   # keep more of the failing log on screen
#   TIMEOUT=7200 scripts/watch-ci.sh    # wait longer than an hour before giving up
#
# **Every filter goes through `gh --jq` rather than a `jq` on the PATH.** `gh` embeds one; Git Bash
# on Windows ships no `jq` at all, and a watcher that only runs on two of the three machines this
# project targets is not one.
#
# **The run is picked by commit, never by recency.** `gh run list --limit 1` answers with whatever
# ran last on the branch, which between `gh workflow run` and the run appearing is the *previous*
# push — a watcher that exits 0 on the last green run instead of waiting for this one is worse than
# no watcher.
#
# **The reader is usually an agent, and it is charged for every line.** Three things follow from
# that. The full log goes to a file and only an extract is printed, because `--log-failed` on a
# three-OS matrix is tens of thousands of lines while the errors inside it are a few dozen. A
# failure that reads the same on two runners is printed once and named twice, because a matrix
# mostly fails the same way everywhere and the second copy buys nothing. And nothing is cut
# silently: a trimmed extract says by how much, since one that stops early without admitting it
# reads exactly like one that had nothing more to give.
#
# **A full run here takes longer than an agent's foreground command may.** Waiting fifteen minutes
# and then being killed at ten pays for the wait and learns nothing, so an agent should either run
# this in the background or poll it with `--once`, which asks, answers and exits.
#
# Exit status is CI's: 0 when the run succeeded, 1 when it did not, and — under `--once` only —
# 2 when it has not finished yet. A misuse of the script itself is 64, so that it cannot be read as
# a report about CI.

set -uo pipefail

interval="${INTERVAL:-30}"
timeout="${TIMEOUT:-3600}"
extract_lines="${EXTRACT_LINES:-200}"
branch="$(git branch --show-current)"

once=0
run=""
for arg in "$@"; do
  case "$arg" in
    --once) once=1 ;;
    -*) echo "unknown option: $arg" >&2; exit 64 ;;
    *) run="$arg" ;;
  esac
done

if [ -z "$run" ]; then
  sha="$(git rev-parse HEAD)"
  find_run() {
    gh run list --branch "$branch" --limit 20 \
      --json databaseId,headSha --jq ".[] | select(.headSha == \"$sha\") | .databaseId" \
      | head -n 1
  }

  # A requested run takes a few seconds to exist. Wait for it rather than answering about an older
  # one — but not under `--once`, whose whole promise is that it returns now.
  run="$(find_run)"
  if [ "$once" -eq 0 ]; then
    waited=0
    while [ -z "$run" ] && [ "$waited" -lt 120 ]; do
      [ "$waited" -eq 0 ] && echo "waiting for a run on ${sha:0:7}…"
      sleep 10
      waited=$((waited + 10))
      run="$(find_run)"
    done
  fi

  if [ -z "$run" ]; then
    echo "no run for ${sha:0:7} on $branch — request one with:" >&2
    echo "  gh workflow run ci.yml --ref $branch" >&2
    exit 1
  fi
fi

repository="$(gh repo view --json nameWithOwner --jq .nameWithOwner)"
echo "watching run $run on $branch — https://github.com/$repository/actions/runs/$run"

# Jobs already reported, so a poll prints what changed rather than the whole table again.
reported=""
status=""
total="?"
settled=0

poll() {
  # One call, not two: the status and job-count lines come out first, the settled jobs after them.
  snapshot="$(gh run view "$run" --json status,jobs \
    --jq '"status\t" + .status, "total\t" + (.jobs | length | tostring),
          (.jobs[] | select(.status == "completed") | .conclusion + "\t" + .name)')"

  settled=0
  while IFS=$'\t' read -r field name; do
    [ -z "$name" ] && continue
    case "$field" in
      status) status="$name"; continue ;;
      total) total="$name"; continue ;;
    esac

    settled=$((settled + 1))
    case "$reported" in *"|$name|"*) continue ;; esac
    reported="$reported|$name|"

    case "$field" in
      success) printf '  ok       %s\n' "$name" ;;
      skipped) printf '  skipped  %s\n' "$name" ;;
      cancelled) printf '  stopped  %s\n' "$name" ;;
      *) printf '  FAILED   %s\n' "$name" ;;
    esac
  done <<EOF
$snapshot
EOF
}

if [ "$once" -eq 1 ]; then
  poll
  if [ "$status" != "completed" ]; then
    echo "run $run: $status — $settled/$total jobs settled"
    exit 2
  fi
else
  elapsed=0
  while true; do
    poll
    [ "$status" = "completed" ] && break

    if [ "$elapsed" -ge "$timeout" ]; then
      echo "run $run still $status after ${timeout}s — giving up on watching, the run itself is fine" >&2
      exit 1
    fi

    sleep "$interval"
    elapsed=$((elapsed + interval))
  done
fi

conclusion="$(gh run view "$run" --json conclusion --jq '.conclusion')"

if [ "$conclusion" = "success" ]; then
  echo "run $run: success"
  exit 0
fi

echo
echo "run $run: $conclusion — the failing steps:"
echo

log="${TMPDIR:-/tmp}/mixengine-ci-$run.log"

# `--log-failed` is the whole reason this is a script rather than `gh run watch`: it prints the log
# of the steps that failed and nothing else. It lands in a file, not on the terminal.
gh run view "$run" --log-failed >"$log" 2>/dev/null

if [ ! -s "$log" ]; then
  # A job killed at setup, cancelled on timeout, or lost with its runner fails with no failing step,
  # and `--log-failed` has nothing to say. The tail of the job's own log does.
  for job in $(gh run view "$run" --json jobs \
    --jq '.jobs[] | select(.conclusion != "success" and .conclusion != "skipped") | .databaseId'); do
    gh run view "$run" --job "$job" --log 2>/dev/null | tail -n 80 >>"$log"
  done
fi

if [ ! -s "$log" ]; then
  echo "  (no log — read it at https://github.com/$repository/actions/runs/$run)"
  exit 1
fi

# Strip gh's `job<TAB>step<TAB>timestamp ` prefix — the first line of every step puts a BOM in front
# of that timestamp — and the colouring `CARGO_TERM_COLOR: always` puts in. Actions stores that
# colouring as the literal text `^[[1m`, four characters and not an escape byte, so both forms go.
# Then keep the lines that name a failure and the few that explain it, and drop the step preamble
# `-A` drags in behind them.
#
# Every pattern that could appear inside a Rust path is anchored to the start of the line:
# unanchored, `error:` matches every `test error::tests::… ok` line in a passing suite, and each one
# drags six lines of context after it. That one missing `^` was most of what this extract used to
# say.
failure_re='##\[error\]|^error(\[E[0-9]+\])?:|^Error: |panicked at|assertion.*failed|^ *--> |^test result: FAILED|^failures:|Process completed with exit code'

clean_job() {
  awk -F'\t' -v job="$1" '$1 == job' "$log" \
    | sed -E $'s/^[^\t]*\t[^\t]*\t(\xef\xbb\xbf)?[0-9T:.Z-]+ //; s/\x1b\\[[0-9;]*[A-Za-z]//g; s/\\^\\[\\[[0-9;]*[A-Za-z]//g'
}

extract_job() {
  clean_job "$1" \
    | grep -aE "$failure_re" -A 6 \
    | grep -avE '^--$|^##\[(end)?group\]|^shell: |^env:$|^ +[A-Z_][A-Z_0-9]*: '
}

# What makes two failures the same failure is what the runners *said*, not where they said it: the
# lines that named the failure and the one that explains each, with the digits and the path
# separator taken out so a pid, an elapsed second, a `\` and a `/` cannot make one failure look like
# two. Deliberately not the six lines of context the extract prints — on this matrix those are the
# daemon's own log, and a temp directory that differs per runner would defeat every comparison.
fingerprint_job() {
  clean_job "$1" \
    | grep -aE "$failure_re" -A 1 \
    | tr '\134' '/' \
    | sed -E 's/[0-9]+//g' \
    | sort -u | cksum | cut -d' ' -f1
}

body="$log.extract"
: >"$body"
seen=""

# A `while` fed by a pipe is a subshell on some shells and `seen` would not survive it, so the job
# names are read from a here-document instead.
jobs_in_log="$(cut -f1 "$log" | sort -u)"

while IFS= read -r job; do
  [ -z "$job" ] && continue
  block="$(extract_job "$job")"
  [ -z "$block" ] && continue

  fp="$(fingerprint_job "$job")"
  case "$seen" in
    *"|$fp="*)
      twin="${seen#*|$fp=}"
      twin="${twin%%|*}"
      printf '=== %s — the same failure as %s ===\n\n' "$job" "$twin" >>"$body"
      continue
      ;;
  esac

  seen="$seen|$fp=$job|"
  printf '=== %s ===\n' "$job" >>"$body"
  printf '%s\n\n' "$block" >>"$body"
done <<EOF
$jobs_in_log
EOF

lines="$(wc -l <"$body")"
if [ "$lines" -gt "$extract_lines" ]; then
  head -n "$extract_lines" "$body"
  echo "… $((lines - extract_lines)) more lines cut — raise EXTRACT_LINES or read the full log"
else
  cat "$body"
fi

echo "full log: $log"

exit 1
