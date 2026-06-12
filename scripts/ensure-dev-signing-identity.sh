#!/usr/bin/env bash
#
# Create a STABLE self-signed code-signing identity for local development.
#
# Why: macOS keys the Accessibility (and Microphone) TCC grant to an app's
# *designated requirement*. For an ad-hoc-signed app that requirement is the
# binary's cdhash, which changes on every rebuild — so the grant breaks and the
# app reads "Not granted" even though System Settings still shows it toggled on.
# Signing with a STABLE identity makes the requirement identity-based instead,
# so the grant survives rebuilds. A self-signed cert is enough for this (the
# cert never has to be trusted by Gatekeeper — only stable).
#
# Run this ONCE:
#   scripts/ensure-dev-signing-identity.sh
#
# Then `scripts/bundle-macos.sh` auto-detects the identity and signs with it.
# Idempotent: re-running reuses the existing cert (re-creating it would rotate
# the requirement and break the grant again — exactly what we're avoiding).
#
# Prints the identity name to stdout; diagnostics go to stderr.

set -euo pipefail

# macOS-only.
if [[ "$(uname)" != "Darwin" ]]; then
  echo "ensure-dev-signing-identity.sh: macOS only — nothing to do." >&2
  exit 0
fi

# Pin macOS's system LibreSSL: a Homebrew/conda OpenSSL 3 on PATH exports
# PKCS#12 with AES-256/SHA-256, which Apple's `security import` rejects with
# "MAC verification failed". LibreSSL emits the legacy 3DES/SHA-1 format Apple
# accepts, and is always present at this path on macOS.
OPENSSL="/usr/bin/openssl"

IDENTITY_CN="Holler Dev Self-Signed"
KEYCHAIN="$HOME/Library/Keychains/holler-dev.keychain-db"
# Protects only this throwaway signing keychain; lets us set the key partition
# list non-interactively without ever touching the user's login keychain.
KEYCHAIN_PW="holler-dev"
P12_PW="holler"

# Already present anywhere in the user's keychains? Reuse it — rotating the cert
# would change the designated requirement and re-break the TCC grant. Note: no
# `-v` (valid-only) here — a self-signed cert is intentionally untrusted
# (CSSMERR_TP_NOT_TRUSTED), which `-v` would hide, so we'd never detect it and
# would keep importing duplicates. Trust is irrelevant: TCC keys the grant on
# the cert's leaf hash, and codesign signs fine with an untrusted identity.
if security find-identity -p codesigning 2>/dev/null | grep -q "$IDENTITY_CN"; then
  echo "==> Reusing existing identity: $IDENTITY_CN" >&2
  echo "$IDENTITY_CN"
  exit 0
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "==> Generating self-signed code-signing certificate" >&2
# LibreSSL (macOS /usr/bin/openssl) supports -addext; the codeSigning EKU is
# what makes the cert usable as a signing identity.
"$OPENSSL" req -x509 -newkey rsa:2048 -nodes \
  -keyout "$WORK/key.pem" -out "$WORK/cert.pem" \
  -days 3650 -subj "/CN=$IDENTITY_CN" \
  -addext "basicConstraints=critical,CA:false" \
  -addext "keyUsage=critical,digitalSignature" \
  -addext "extendedKeyUsage=critical,codeSigning" 2>/dev/null

"$OPENSSL" pkcs12 -export -out "$WORK/identity.p12" \
  -inkey "$WORK/key.pem" -in "$WORK/cert.pem" \
  -passout "pass:$P12_PW" 2>/dev/null

echo "==> Importing into a dedicated keychain ($KEYCHAIN)" >&2
# `|| true`: create is a no-op error if the keychain already exists (e.g. a
# previous partial run); the import/partition-list steps below still apply.
security create-keychain -p "$KEYCHAIN_PW" "$KEYCHAIN" 2>/dev/null || true
security set-keychain-settings "$KEYCHAIN" # no auto-lock timeout
security unlock-keychain -p "$KEYCHAIN_PW" "$KEYCHAIN"
security import "$WORK/identity.p12" -k "$KEYCHAIN" -P "$P12_PW" \
  -T /usr/bin/codesign -A
# Let codesign use the private key without an interactive "allow" prompt.
security set-key-partition-list -S apple-tool:,apple:,codesign: \
  -s -k "$KEYCHAIN_PW" "$KEYCHAIN" >/dev/null

# Add the keychain to the user search list (preserving the existing ones) so
# codesign / find-identity can see the new identity. Rebuild the list with any
# prior copies of our keychain stripped, then append it exactly once — appending
# blindly would stack duplicate entries (each one re-lists the same identity).
OTHERS=()
while IFS= read -r kc; do
  kc="${kc//\"/}"            # strip the surrounding quotes
  kc="${kc//[[:space:]]/}"   # strip leading indentation
  [[ -z "$kc" || "$kc" == "$KEYCHAIN" ]] && continue
  OTHERS+=("$kc")
done < <(security list-keychains -d user)
security list-keychains -d user -s "${OTHERS[@]}" "$KEYCHAIN"

echo "==> Done. bundle-macos.sh will now sign with: $IDENTITY_CN" >&2
echo "$IDENTITY_CN"
