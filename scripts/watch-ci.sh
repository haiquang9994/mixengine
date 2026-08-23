#!/usr/bin/env bash
# Watch the CI run this branch asked for, and say what failed rather than that something did.
#
# CI fires by itself on `master` and on nothing else — see
# `.claude/operations/build-and-release.md`. Every other branch pushes and then requests a run, and
# this is the loop that waits for the answer: it polls `gh run view`, prints each job as it settles,
# and on a failure prints the failing step's log so the next thing to read is the error and not a
# URL.
#
#   scripts/watch-ci.sh                 # the newest run on the current branch
#   scripts/watch-ci.sh 32617405595     # a run by id
#   INTERVAL=60 scripts/watch-ci.sh     # poll less often
#
# **Every filter goes through `gh --jq` rather than a `jq` on the PATH.** `gh` embeds one; Git Bash
# on Windows ships no `jq` at all, and a watcher that only runs on two of the three machines this
# project targets is not one.
#
# Exit status is CI's: 0 when the run succeeded, 1 when it did not.

set -uo pipefail

interval="${INTERVAL:-30}"
branch="$(git branch --show-current)"

run="${1:-}"
if [ -z "$run" ]; then
  run="$(gh run list --branch "$branch" --limit 1 --json databaseId --jq '.[0].databaseId')"
  if [ -z "$run" ] || [ "$run" = "null" ]; then
    echo "no run on $branch yet — request one with:" >&2
    echo "  gh workflow run ci.yml --ref $branch" >&2
    exit 1
  fi
fi

repository="$(gh repo view --json nameWithOwner --jq .nameWithOwner)"
echo "watching run $run on $branch — https://github.com/$repository/actions/runs/$run"

# Jobs already reported, so a poll prints what changed rather than the whole table again.
reported=""

while true; do
  status="$(gh run view "$run" --json status --jq '.status')"
  settled="$(gh run view "$run" --json jobs \
    --jq '.jobs[] | select(.status == "completed") | .conclusion + "\t" + .name')"

  while IFS=$'\t' read -r conclusion name; do
    [ -z "$name" ] && continue
    case "$reported" in *"|$name|"*) continue ;; esac
    reported="$reported|$name|"

    case "$conclusion" in
      success) printf '  ok       %s\n' "$name" ;;
      skipped) printf '  skipped  %s\n' "$name" ;;
      cancelled) printf '  stopped  %s\n' "$name" ;;
      *) printf '  FAILED   %s\n' "$name" ;;
    esac
  done <<EOF
$settled
EOF

  if [ "$status" = "completed" ]; then
    break
  fi

  sleep "$interval"
done

conclusion="$(gh run view "$run" --json conclusion --jq '.conclusion')"

if [ "$conclusion" = "success" ]; then
  echo "run $run: success"
  exit 0
fi

echo
echo "run $run: $conclusion — the failing steps:"
echo

# `--log-failed` is the whole reason this is a script rather than `gh run watch`: it prints the log
# of the steps that failed and nothing else, which is what a person reads next.
gh run view "$run" --log-failed || true

exit 1
