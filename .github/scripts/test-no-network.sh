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

  # `--all-targets` silently excludes doc tests, so they get their own invocation — inside the same
  # namespace, otherwise a doc example could reach the network unnoticed.
  cargo test --workspace --all-targets --all-features --locked --offline
  cargo test --workspace --all-features --locked --offline --doc

  # The one `#[ignore]`d suite this job runs: the Caddy recipe against a real Caddy, which the
  # workflow fetched before the network was taken away. Inside the namespace like everything else —
  # a server on loopback needs no route out, and running it outside would leave the one test that
  # binds a port as the one test nothing stops from reaching the internet.
  if [ -n "${MIXENGINE_CADDY_PACKAGE:-}" ]; then
    cargo test -p mixengine-cli --test caddy --locked --offline -- --ignored
  else
    echo "::warning title=No Caddy::MIXENGINE_CADDY_PACKAGE is not set, so the Caddy recipe was not judged against a real server on this leg."
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
  for name in CARGO CARGO_HOME RUSTUP_HOME CARGO_NET_OFFLINE CARGO_TERM_COLOR CARGO_INCREMENTAL RUST_BACKTRACE MIXENGINE_CADDY_PACKAGE; do
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
