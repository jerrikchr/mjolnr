#!/bin/sh
# Install mjolnr and sign it with a STABLE identity.
#
# Why: macOS keychain ACLs are keyed to the binary's code signature. Plain
# `cargo install` (and `codesign -s -`) produce a fresh ad-hoc signature per
# build, so the keychain treats every rebuild as an unknown app and asks for
# the login password again. Signing with a named identity keeps the identity
# constant across rebuilds, so one "Always Allow" per credential holds forever.
#
# The identity below is the local Apple Development certificate (from Xcode).
# List candidates with:  security find-identity -v -p codesigning
set -eu

IDENTITY="${MJOLNR_SIGN_IDENTITY:-${SMED_SIGN_IDENTITY:-D507F58CF5AD58A6472534ED911C4E9F02D66642}}"

cargo install --path "$(dirname "$0")/.." --force
codesign -f -s "$IDENTITY" "$HOME/.cargo/bin/mjolnr"
codesign -dv "$HOME/.cargo/bin/mjolnr" 2>&1 | sed -n 's/^Authority=/signed by: /p' | head -1
echo "installed and signed: $HOME/.cargo/bin/mjolnr"
