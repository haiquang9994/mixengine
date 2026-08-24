#!/usr/bin/env bash
#
# The credential-store tests against a Linux with no secret service — the machine T15b exists for,
# and the one no runner is by accident.
#
# **This is the only leg that walks the absent branch.** Everywhere else the store is there: the
# `test` job installs `gnome-keyring` on purpose so that Linux gets the coverage Windows and macOS
# get for free, which leaves the answer a machine *without* one gets as a path every green run
# steps past. That is how T15b's bug lived long enough to be found by a stack trace.
#
# `MIXENGINE_TEST_NO_KEYRING=1` is what makes each round an assertion rather than a skip: with it
# set, `crates/mixengine-platform/tests/secrets.rs` fails if it finds a store, so a round whose
# sabotage did not work reports that instead of passing quietly.
#
# The three shapes below are the three D-Bus error names `linux/secrets.rs` matches, and they are
# genuinely different machines rather than three spellings of one — see that module.
set -euo pipefail

cd -- "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/../.."

# `--all-features` matches the job's own "Build tests" step, so these three rounds reuse what it
# already compiled instead of paying for a second feature resolution of the whole crate.
suite=(cargo test -p mixengine-platform --test secrets --all-features --locked --offline)

# A session bus of our own that can activate nothing: no `<servicedir>` at all, so the name
# `org.freedesktop.secrets` has no owner and none can be started. Without this the runner's own bus
# would activate the `gnome-keyring` the `test` job installed, and the round would prove the
# opposite of what it is for.
config=$(mktemp)
cat > "$config" <<'XML'
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=/tmp</listen>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
XML

pid_file=$(mktemp)
address=$(dbus-daemon --config-file="$config" --print-address --fork --print-pid=3 3>"$pid_file")

cleanup() {
  if [ -s "$pid_file" ]; then
    kill "$(cat "$pid_file")" 2>/dev/null || true
  fi
  rm -f "$config" "$pid_file"
}
trap cleanup EXIT

export MIXENGINE_TEST_NO_KEYRING=1
export CARGO_NET_OFFLINE=true

# `org.freedesktop.DBus.Error.ServiceUnknown` — a bus with nobody serving secrets on it. This is the
# shape the CI run that opened T15b actually produced.
echo "--- a session bus with no secret service on it ---"
DBUS_SESSION_BUS_ADDRESS="$address" "${suite[@]}"

# `org.freedesktop.DBus.Error.NotSupported` — no bus to reach and none that can be started. **The
# shape a headless machine actually hits**, and the one a match on the name above would have missed:
# it fails a step earlier, at the bus, and never gets far enough to be told there is no provider.
echo "--- a login with no session bus at all ---"
env -u DBUS_SESSION_BUS_ADDRESS -u XDG_RUNTIME_DIR -u DISPLAY "${suite[@]}"

# `org.freedesktop.DBus.Error.FileNotFound` — a stale address, which is what a `systemd` unit or a
# `cron` job inherits far more often than a person meets it.
echo "--- a session bus address pointing at nothing ---"
DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent/bus "${suite[@]}"
