#!/usr/bin/env bash
#
# Does this machine's Firefox trust a certificate authority in the user's own trust store?
#
# The open half of T49b's D14. On Linux the answer is no — Firefox and Chrome read NSS databases,
# which is what `mixengine-platform`'s `browsers` module writes into. On Windows the answer was
# measured on 2026-08-25 and is yes: an authority placed only in `Cert:\CurrentUser\Root` produced an
# ordinary padlock in Firefox, Chrome and Edge alike. **macOS is unmeasured**, and this script is the
# measurement, written to be run rather than described — the machine with Firefox on it is not the
# machine this repository is usually worked on, and a note in somebody's local notes does not travel.
#
# ## Why it is shaped like this
#
# **A handshake and never a certificate list.** Three indirect measurements were made on Windows
# first — searching for a self-signed root that only the machine store held, searching for Microsoft
# roots outside the two Mozilla ships, reading the pref — and all three pointed the wrong way, for
# one reason: **Firefox's Certificate Manager does not list enterprise roots at all.** Its Authorities
# tab shows Mozilla's built-in set and nothing more. Looking at a list cannot answer this question.
#
# **A control is mandatory.** A red padlock has two explanations — the browser does not read the
# store, or the certificate was built wrong — and they are not distinguishable without something that
# certainly reads the store. Here that is `security verify-cert`, which goes through the same
# Security framework Safari and Chrome do. If the control fails, the run says so and stops: a
# measurement whose apparatus is broken must not be reported as a result.
#
# **Firefox must be fully quit first.** Trust anchors are read at start-up. On macOS closing the
# window does not quit the application — it takes Cmd-Q — and skipping it produces a false negative
# that looks exactly like a real one.
#
# Nothing here needs `sudo`: the authority goes into the login keychain, which belongs to the user.
# Everything is removed again on exit, including on Ctrl-C, and the certificates expire in two days
# so that a run that is killed in a way `trap` cannot catch still cannot leave a standing root.

set -euo pipefail

readonly PORT=8443
readonly SUBJECT_NAME='MixEngine Probe CA (throwaway)'
readonly KEYCHAIN="${HOME}/Library/Keychains/login.keychain-db"

if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "This probe is macOS-only: the Windows and Linux halves of the question are already" >&2
    echo "answered — see the T49b entry in .claude/roadmap/phase-5-https.md." >&2
    exit 2
fi

workspace="$(mktemp -d)"
server_pid=""
installed=""

cleanup() {
    local status=$?

    if [[ -n "$server_pid" ]] && kill -0 "$server_pid" 2>/dev/null; then
        kill "$server_pid" 2>/dev/null || true
        wait "$server_pid" 2>/dev/null || true
    fi

    if [[ -n "$installed" ]]; then
        echo
        echo "Removing the throwaway authority. macOS asks for confirmation on this too."
        # `-t` takes the trust settings with the certificate; without it a trust entry for a
        # certificate that is gone stays behind in the keychain.
        security delete-certificate -c "$SUBJECT_NAME" -t "$KEYCHAIN" 2>/dev/null || true
    fi

    rm -rf "$workspace"

    if security find-certificate -c "$SUBJECT_NAME" "$KEYCHAIN" >/dev/null 2>&1; then
        echo "WARNING: something named '$SUBJECT_NAME' is still in $KEYCHAIN." >&2
        echo "Remove it by hand in Keychain Access before trusting this machine again." >&2
        exit 1
    fi

    echo "Cleaned up: nothing named '$SUBJECT_NAME' is left in the keychain."
    exit "$status"
}
trap cleanup EXIT INT TERM

# --- the authority and the leaf ------------------------------------------------------------------
#
# Shaped like the ones `mixengine_core::certs` writes: ECDSA P-256, the authority `pathlen:0` so it
# may sign a leaf and not another authority, the leaf `serverAuth` alone. An openssl config file
# rather than `-addext`, because LibreSSL — which is what macOS ships as `openssl` — has not always
# had that flag, and a probe that fails to run tells nobody anything.

cat > "$workspace/ca.cnf" <<'CONFIG'
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no

[dn]
CN = MixEngine Probe CA (throwaway)

[ext]
basicConstraints = critical, CA:true, pathlen:0
keyUsage = critical, keyCertSign, cRLSign
subjectKeyIdentifier = hash
CONFIG

cat > "$workspace/leaf.cnf" <<'CONFIG'
[req]
distinguished_name = dn
req_extensions = ext
prompt = no

[dn]
CN = localhost

[ext]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = serverAuth
subjectAltName = DNS:localhost
CONFIG

echo "Building a throwaway authority and one leaf for localhost..."

openssl ecparam -name prime256v1 -genkey -noout -out "$workspace/ca.key" 2>/dev/null
openssl req -new -x509 -key "$workspace/ca.key" -out "$workspace/ca.crt" \
    -days 2 -config "$workspace/ca.cnf" 2>/dev/null

openssl ecparam -name prime256v1 -genkey -noout -out "$workspace/leaf.key" 2>/dev/null
openssl req -new -key "$workspace/leaf.key" -out "$workspace/leaf.csr" \
    -config "$workspace/leaf.cnf" 2>/dev/null
openssl x509 -req -in "$workspace/leaf.csr" \
    -CA "$workspace/ca.crt" -CAkey "$workspace/ca.key" -CAcreateserial \
    -out "$workspace/leaf.crt" -days 2 \
    -extfile "$workspace/leaf.cnf" -extensions ext 2>/dev/null

# --- into the user's own keychain ----------------------------------------------------------------

echo
echo "Adding it to your login keychain. macOS will ask you to confirm — this is not sudo,"
echo "and nothing outside your own account is touched."
security add-trusted-cert -r trustRoot -k "$KEYCHAIN" "$workspace/ca.crt"
installed=1

# --- the control ----------------------------------------------------------------------------------
#
# `security verify-cert` goes through the Security framework, which is what Safari and Chrome read.
# If this says no, the apparatus is broken and no browser result would mean anything.

echo
echo "Control: asking macOS itself whether it now trusts the leaf..."
if ! security verify-cert -c "$workspace/leaf.crt" -p ssl -s localhost 2>&1; then
    echo >&2
    echo "CONTROL FAILED: macOS does not trust this chain, so the probe itself is wrong." >&2
    echo "Nothing can be concluded about Firefox from this run. Do not report a result." >&2
    exit 1
fi
echo "Control passed: the Security framework trusts it."

# --- serve it -------------------------------------------------------------------------------------

openssl s_server -accept "$PORT" -cert "$workspace/leaf.crt" -key "$workspace/leaf.key" \
    -www -quiet >/dev/null 2>&1 &
server_pid=$!

sleep 1
if ! kill -0 "$server_pid" 2>/dev/null; then
    echo "The TLS listener did not start — is something already on port $PORT?" >&2
    exit 1
fi

cat <<INSTRUCTIONS

The listener is up on https://localhost:${PORT}

  1. QUIT Firefox completely — Cmd-Q, not just closing the window. Trust anchors are read at
     start-up, and skipping this produces a false negative that looks like a real one.
  2. Open Firefox again and go to  https://localhost:${PORT}
  3. Do the same in Safari, which certainly reads the keychain, as a second control.

  Firefox trusts it  -> macOS needs no NSS handling either; Browsers::NotSearched is correct there,
                        and T49b's D14 is closed.
  Firefox refuses    -> and Safari accepts: Firefox on macOS keeps its own store, T49b has a real
                        gap there, and the fix is expensive — macOS ships no NSS certutil.
  Both refuse        -> the apparatus is wrong despite the control. Report nothing.

Press Enter when you have looked, and everything will be removed.
INSTRUCTIONS

read -r _
