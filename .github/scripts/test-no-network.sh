#!/usr/bin/env bash
# Run the workspace test suite on Linux CI with no route to the outside world.
#
# .claude/standards/testing.md forbids network access in tests outside of MockRegistry, which serves
# its index and artifacts over loopback. This script enforces that rule by putting the suite in a
# private network namespace containing nothing but `lo`: an accidental outbound connection fails in
# CI instead of turning into a flaky test that only breaks when GitHub is slow.
#
# Two mechanisms are tried, in order of least privilege. Each is probed by running the exact thing it
# will have to do — creating the namespace *and* bringing loopback up — because a namespace without
# working loopback would break MockRegistry rather than block the network. If neither mechanism is
# available the suite still runs, since a missing sandbox is an environment problem and not a test
# failure, but the job log says so loudly.

set -euo pipefail

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
script_path="$script_dir/$(basename -- "${BASH_SOURCE[0]}")"
cd -- "$script_dir/../.."

# Whether anything owns the secret service's name on this run's session bus.
#
# `gnome-keyring-daemon` returns as soon as it has forked, so a bus with nothing on that name yet is
# the ordinary state for a moment — and a permanent one if the daemon died on the way up. Asking is
# one D-Bus round trip and needs no package beyond the `dbus` this job already installs.
secret_service_is_answering() {
  # A runner without `dbus-send` is one this cannot ask, and an unanswerable question is not a
  # missing store: saying yes here leaves the script behaving exactly as it did before this check
  # existed, rather than inventing a failure out of a tool that is not installed.
  command -v dbus-send >/dev/null 2>&1 || return 0

  dbus-send --session --print-reply --dest=org.freedesktop.DBus \
    /org/freedesktop/DBus org.freedesktop.DBus.NameHasOwner \
    string:org.freedesktop.secrets 2>/dev/null | grep -q "boolean true"
}

# What a failing run is asked afterwards, because the interesting failures on this leg have twice not
# been about the code.
#
# **Measured, on a run of `master` that had passed on its branch an hour earlier**: the four
# credential-store tests failed together with `Message recipient disconnected from message bus
# without replying`, a re-run of the same commit was green, and the same suite reproduced it once in
# two tries on a local Linux. Nothing was established beyond that: it is load-dependent rather than
# time-dependent (a store left alone for three minutes answers perfectly), and whether the daemon
# dies or only drops the connection is exactly what nobody could tell from the log, because the log
# carries four D-Bus sentences and nothing about the machine they came from. So the run now says.
aftermath() {
  if [ "${MIXENGINE_TEST_KEYRING:-}" = "1" ]; then
    if secret_service_is_answering; then
      echo "The secret service was still on the bus when this suite ended."
    else
      echo "::warning title=The secret service went away::org.freedesktop.secrets had no owner when the suite ended. Whatever the credential-store tests reported is then about a store that stopped being there, not about the code."
    fi
  fi

  # The kernel is the only place a daemon dying of its own accord is written down, and reading it is
  # what separates that from a daemon that was fine. Neither reading is a failure of this script.
  echo "--- the last of the kernel log ---"
  { dmesg 2>/dev/null || sudo -n dmesg 2>/dev/null || echo "unreadable on this runner"; } | tail -40
}

# Second entry point: we are inside the namespace. One thing is still missing — a credential store.
#
# A stock runner has a session bus with nothing serving `org.freedesktop.secrets` on it, and that is
# not the same as having no store: `crates/mixengine-platform/tests/secrets.rs` skips only on
# `Error::UnsupportedPlatform` and fails on anything else, deliberately, so that a store which
# quietly forgets cannot pass as a store that is absent. Supplying a real one is what gives Linux the
# coverage Windows and macOS get for free from the Credential Manager and the Keychain.
if [ "${MIXENGINE_TEST_ISOLATED:-}" = "1" ]; then
  if [ "${MIXENGINE_TEST_KEYRING:-}" != "1" ] \
    && command -v dbus-run-session >/dev/null 2>&1 \
    && command -v gnome-keyring-daemon >/dev/null 2>&1; then
    echo "Credential store: gnome-keyring, on a session bus belonging to this run alone."

    # Third entry point. The empty password is what makes it unattended: it creates the login keyring
    # if there is none and unlocks it either way, so the secret service is answering by the time
    # cargo starts. Everything dies with the session bus, so no daemon outlives the job and no
    # keyring is left on the machine.
    exec dbus-run-session -- sh -c \
      'printf "" | gnome-keyring-daemon --unlock --components=secrets >/dev/null || exit 1
       exec env MIXENGINE_TEST_KEYRING=1 bash "$1"' sh "$script_path"
  fi

  if [ "${MIXENGINE_TEST_KEYRING:-}" != "1" ]; then
    echo "::warning title=No credential store::dbus-run-session or gnome-keyring is missing, so the secret-service tests will fail rather than skip — a session bus with no provider on it is a store that is not there, not a machine without one."
  fi

  # Started is not answering, and the difference is worth five seconds here rather than a quarter of
  # an hour of reading afterwards: a store that never arrived reaches `secrets.rs` as a store that is
  # there and refusing, which is the one thing those tests are built never to forgive.
  if [ "${MIXENGINE_TEST_KEYRING:-}" = "1" ]; then
    waited=0
    until secret_service_is_answering; do
      waited=$((waited + 1))

      if [ "$waited" -gt 50 ]; then
        echo "::error title=No secret service::gnome-keyring was started and org.freedesktop.secrets never appeared on this run's session bus, so the credential-store tests would have been judging a store that is not there."
        exit 1
      fi

      sleep 0.1
    done
  fi

  # `--all-targets` silently excludes doc tests, so they get their own invocation — inside the same
  # namespace, otherwise a doc example could reach the network unnoticed.
  if ! cargo test --workspace --all-targets --all-features --locked --offline; then
    aftermath
    exit 1
  fi

  if ! cargo test --workspace --all-features --locked --offline --doc; then
    aftermath
    exit 1
  fi

  # The one `#[ignore]`d suite this job runs: the Caddy recipe against a real Caddy, which the
  # workflow fetched before the network was taken away. Inside the namespace like everything else —
  # a server on loopback needs no route out, and running it outside would leave the one test that
  # binds a port as the one test nothing stops from reaching the internet.
  if [ -n "${MIXENGINE_CADDY_PACKAGE:-}" ]; then
    cargo test -p mixengine-cli --test caddy --locked --offline -- --ignored
  else
    echo "::warning title=No Caddy::MIXENGINE_CADDY_PACKAGE is not set, so the Caddy recipe was not judged against a real server on this leg."
  fi

  # And the php-fpm recipe against a real PHP (T32), on the same reasoning: the pool listens on a
  # Unix socket in the home directory, and a FastCGI request to it needs no route out either.
  if [ -n "${MIXENGINE_PHP_RUNTIME:-}" ]; then
    cargo test -p mixengine-cli --test php_fpm --locked --offline -- --ignored
  else
    echo "::warning title=No PHP::MIXENGINE_PHP_RUNTIME is not set, so the php-fpm recipe was not judged against a real PHP on this leg."
  fi

  exit 0
fi

# A new user namespace carries full capabilities *inside itself*, which is all that configuring its
# private loopback needs — no sudo involved. Hardened kernels (Ubuntu restricts unprivileged user
# namespaces through AppArmor) may refuse it, hence the probe.
#
# `--map-current-user` is tried first so the suite keeps running under the real uid. Under
# `--map-root-user` the tests would see themselves as uid 0, which would quietly invalidate every
# assertion about file permissions and about refusing to run as root (T7, T40).
for mapping in --map-current-user --map-root-user; do
  if unshare --user "$mapping" --net -- sh -c 'ip link set lo up' >/dev/null 2>&1; then
    echo "Network isolation: unprivileged user + network namespace ($mapping)."
    exec unshare --user "$mapping" --net -- \
      sh -c 'ip link set lo up || exit 1; exec "$@"' \
      sh env MIXENGINE_TEST_ISOLATED=1 bash "$script_path"
  fi
done

# Fallback: root creates the namespace, then hands execution straight back to the invoking user so
# nothing in the build tree ends up owned by root and poisoning the cache for the next run.
#
# The environment is passed explicitly rather than with `sudo --preserve-env`, which sudoers policy
# is free to reject and which `secure_path` would override for PATH anyway.
if sudo -n unshare --net -- sh -c 'ip link set lo up && command -v runuser' >/dev/null 2>&1; then
  echo "Network isolation: privileged network namespace, tests dropped back to $(id -un)."

  env_args=("PATH=$PATH" "HOME=$HOME" "MIXENGINE_TEST_ISOLATED=1")
  # These are forwarded only if they are set — on a stock runner most are not, and cargo derives its
  # paths from HOME. CARGO_HOME matters most: losing it would send cargo looking for the registry in
  # the default location, find nothing there, and fail instantly because there is no network to fall
  # back on. CARGO_NET_OFFLINE matters for the same reason, one level down: `cargo metadata`, which
  # the layering test spawns, inherits no `--offline` flag of ours.
  for name in CARGO CARGO_HOME RUSTUP_HOME CARGO_NET_OFFLINE CARGO_TERM_COLOR CARGO_INCREMENTAL RUST_BACKTRACE MIXENGINE_CADDY_PACKAGE MIXENGINE_PHP_RUNTIME; do
    if [ -n "${!name-}" ]; then
      env_args+=("$name=${!name}")
    fi
  done

  exec sudo -n unshare --net -- \
    sh -c 'ip link set lo up || exit 1; user="$1"; shift; exec runuser -u "$user" -- "$@"' \
    sh "$(id -un)" env "${env_args[@]}" bash "$script_path"
fi

echo "::warning title=No network isolation::Neither unprivileged namespaces nor sudo are available on this runner; the suite ran with network access. Tests reaching the network will not be caught here."
exec env MIXENGINE_TEST_ISOLATED=1 bash "$script_path"
