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
# A stock runner has a session bus with nothing serving `org.freedesktop.secrets` on it, which since
# T15b `crates/mixengine-platform/tests/secrets.rs` reads for what it is — a machine with no
# credential store — and therefore **skips**. Supplying a real one is what gives Linux the coverage
# Windows and macOS get for free from the Credential Manager and the Keychain.
#
# **Which is why the absence of a store must fail this script rather than warn it.** Before T15b a
# runner with no `gnome-keyring` produced eight loud failures; now it produces eight quiet skips, and
# the only thing standing between that and a leg reporting green while proving nothing is the check
# below. The branch those skips take is proved by the workflow's own "no secret service" step, which
# takes the store away on purpose and asserts it is gone.
if [ "${MIXENGINE_TEST_ISOLATED:-}" = "1" ]; then
  if [ "${MIXENGINE_TEST_KEYRING:-}" != "1" ] \
    && command -v dbus-run-session >/dev/null 2>&1 \
    && command -v gnome-keyring-daemon >/dev/null 2>&1; then
    echo "Credential store: gnome-keyring, on a session bus belonging to this run alone."

    # Third entry point, and **the password must not be empty**. `--unlock` reads one from standard
    # input, and this is the difference an empty one makes, measured on a stock Ubuntu 24.04: the
    # daemon starts, `org.freedesktop.secrets` appears on the bus, `NameHasOwner` answers true — and
    # the only collection it owns is `session`, with the `default` alias pointing at nothing. Every
    # store then fails with `Secret Service: no result found`, which reaches
    # `crates/mixengine-platform/tests/secrets.rs` as `UnsupportedPlatform` and is **skipped** — a
    # leg reporting eight passing credential tests while holding no credential store at all. One
    # non-empty password creates `login`, the `default` alias resolves to it, and the same eight
    # tests start storing something. T33's MariaDB suite cannot run at all without it: the root
    # password it generates has exactly one home.
    #
    # The value is a constant on purpose — it is the key to a keyring that exists for the length of
    # one CI job, on a runner that is deleted afterwards. On a machine that already has a login
    # keyring this fails to unlock it rather than replacing it, which is the safe way round and the
    # reason this lives in a CI script rather than in anything a developer runs by habit.
    exec dbus-run-session -- sh -c \
      'printf "mixengine-ci" | gnome-keyring-daemon --unlock --components=secrets >/dev/null || exit 1
       exec env MIXENGINE_TEST_KEYRING=1 bash "$1"' sh "$script_path"
  fi

  if [ "${MIXENGINE_TEST_KEYRING:-}" != "1" ]; then
    echo "::error title=No credential store::dbus-run-session or gnome-keyring is missing, so the credential-store tests would skip rather than run — since T15b a session bus with no provider on it is read as a machine with no store, which is what it is. This leg exists to judge a real one."
    exit 1
  fi

  # Started is not answering, and since T15b this loop is the whole guard rather than a convenience.
  # A `gnome-keyring` that forked and then died leaves its name unowned, which the capability now
  # reads as a machine with no secret service — so `secrets.rs` would **skip** and the leg would go
  # green having judged nothing. Before T15b the same state produced four loud failures and this loop
  # only saved a quarter of an hour of reading them.
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

  # And the other front end through the same arc (T37), which is the parity half of that task: one
  # sequence of assertions in `tests/harness/frontend.rs`, driven by both `caddy.rs` and `nginx.rs`.
  # Inside the namespace for the reason Caddy is — an nginx on loopback needs no route out.
  if [ -n "${MIXENGINE_NGINX_PACKAGE:-}" ]; then
    cargo test -p mixengine-cli --test nginx --locked --offline -- --ignored
  else
    echo "::warning title=No nginx::MIXENGINE_NGINX_PACKAGE is not set, so the nginx recipe was not judged against a real server on this leg."
  fi

  # And the php-fpm recipe against a real PHP (T32), on the same reasoning: the pool listens on a
  # Unix socket in the home directory, and a FastCGI request to it needs no route out either.
  if [ -n "${MIXENGINE_PHP_RUNTIME:-}" ]; then
    cargo test -p mixengine-cli --test php_fpm --locked --offline -- --ignored
  else
    echo "::warning title=No PHP::MIXENGINE_PHP_RUNTIME is not set, so the php-fpm recipe was not judged against a real PHP on this leg."
  fi

  # And the ini set that PHP reads (T28), which needs the same PHP and no route out either: the
  # terminal half runs a shim in the home directory and the pool half is a FastCGI request to a Unix
  # socket in it. **This is the leg that measures `SIGUSR2`** — whether a reload picks up a newly
  # enabled extension is a question only a system with signals can answer.
  if [ -n "${MIXENGINE_PHP_RUNTIME:-}" ]; then
    cargo test -p mixengine-cli --test php_extensions --locked --offline -- --ignored
  else
    echo "::warning title=No PHP::MIXENGINE_PHP_RUNTIME is not set, so the generated ini set was not judged against a real PHP on this leg."
  fi

  # And the MariaDB recipe against a real server (T33). Inside the namespace for the same reason,
  # and inside *this script* for one the other two do not have: the first-run ritual puts the
  # generated root password in the OS credential store and refuses a machine with none, and this is
  # where a `gnome-keyring` is running on a session bus of its own.
  if [ -n "${MIXENGINE_MARIADB_PACKAGE:-}" ]; then
    cargo test -p mixengine-cli --test mariadb --locked --offline -- --ignored --nocapture
  else
    echo "::warning title=No MariaDB::MIXENGINE_MARIADB_PACKAGE is not set, so the MariaDB recipe was not judged against a real server on this leg."
  fi

  # And two of them at once, at two versions (T36). Here rather than beside the suite above for the
  # same reason it is here: two first-run rituals store two generated root passwords, and this is
  # the only place on this leg where there is a store to put them in. Both archives or neither —
  # the whole claim is that the two are different versions, so one of them alone proves nothing this
  # suite is for.
  if [ -n "${MIXENGINE_MARIADB_PACKAGE:-}" ] && [ -n "${MIXENGINE_MARIADB_LEGACY_PACKAGE:-}" ]; then
    cargo test -p mixengine-cli --test instances --locked --offline -- --ignored --nocapture
  else
    echo "::warning title=No second MariaDB::MIXENGINE_MARIADB_LEGACY_PACKAGE is not set, so two instances of one server were not run side by side on this leg."
  fi

  # And the MySQL recipe against a real server (T34c). Inside this script rather than beside it for
  # MariaDB's reason: the ritual stores a generated root password in the OS credential store and
  # refuses a machine with none, and this is where a `gnome-keyring` runs on a session bus of its
  # own.
  if [ -n "${MIXENGINE_MYSQL_PACKAGE:-}" ]; then
    cargo test -p mixengine-cli --test mysql --locked --offline -- --ignored --nocapture
  else
    echo "::warning title=No MySQL::MIXENGINE_MYSQL_PACKAGE is not set, so the MySQL recipe was not judged against a real server on this leg."
  fi

  # And the PostgreSQL recipe against a real server (T34). Inside the namespace for the reason the
  # others are, and inside *this script* for the reason MariaDB is: the first-run ritual puts the
  # generated superuser password in the OS credential store and refuses a machine with none, and
  # this is where a `gnome-keyring` is running on a session bus of its own.
  if [ -n "${MIXENGINE_POSTGRES_PACKAGE:-}" ]; then
    cargo test -p mixengine-cli --test postgres --locked --offline -- --ignored --nocapture
  else
    echo "::warning title=No PostgreSQL::MIXENGINE_POSTGRES_PACKAGE is not set, so the PostgreSQL recipe was not judged against a real server on this leg."
  fi

  # And the two caches (T35), which need the namespace and nothing else in it: neither has a
  # credential to store, and both are spoken to over loopback in their own protocols.
  if [ -n "${MIXENGINE_REDIS_PACKAGE:-}" ]; then
    cargo test -p mixengine-cli --test redis --locked --offline -- --ignored --nocapture
  else
    echo "::warning title=No Redis::MIXENGINE_REDIS_PACKAGE is not set, so the Redis recipe was not judged against a real server on this leg."
  fi

  if [ -n "${MIXENGINE_MEMCACHED_PACKAGE:-}" ]; then
    cargo test -p mixengine-cli --test memcached --locked --offline -- --ignored --nocapture
  else
    echo "::warning title=No memcached::MIXENGINE_MEMCACHED_PACKAGE is not set, so the Memcached recipe was not judged against a real server on this leg."
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
  # paths from HOME. **A package variable left off this list is a leg that reports green having run
  # nothing**: the suite it feeds is `#[ignore]`d, so the block below warns and moves on. T34 added
  # the fourth entry after a run did exactly that; T35 added the fifth and sixth after a run did it
  # again, judging neither cache on this leg while every job went green; T36 added the second
  # MariaDB after a third run did it a third time. The warning is an annotation rather than a log
  # line, which is why the same mistake keeps arriving unnoticed.
  # CARGO_HOME matters most: losing it would send cargo looking for the registry in the default
  # location, find nothing there, and fail instantly because there is no network to fall back on.
  # CARGO_NET_OFFLINE matters for the same reason, one level down: `cargo metadata`, which the
  # layering test spawns, inherits no `--offline` flag of ours.
  for name in CARGO CARGO_HOME RUSTUP_HOME CARGO_NET_OFFLINE CARGO_TERM_COLOR CARGO_INCREMENTAL RUST_BACKTRACE MIXENGINE_CADDY_PACKAGE MIXENGINE_NGINX_PACKAGE MIXENGINE_PHP_RUNTIME MIXENGINE_MARIADB_PACKAGE MIXENGINE_MARIADB_LEGACY_PACKAGE MIXENGINE_MYSQL_PACKAGE MIXENGINE_POSTGRES_PACKAGE MIXENGINE_REDIS_PACKAGE MIXENGINE_MEMCACHED_PACKAGE; do
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
