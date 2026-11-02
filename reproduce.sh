#!/usr/bin/env bash
# One-shot reproduce from zero on NEAR TESTNET (never mainnet):
#   build contract -> install JS deps -> fresh faucet account -> deploy/lock wallet -> agent run -> runs/<id>.{json,md}
# Throwaway keys are written to $SPENDCAP_CREDS_DIR (default: a hidden testnet credentials directory under the home directory),
# which is outside this repo. Nothing secret is written under this directory.
set -euo pipefail
cd "$(dirname "$0")"

for tool in rustup rustc cargo node npm python3; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "Missing prerequisite: $tool. Install Rust through rustup, Node 22+ with npm, and Python 3 before running reproduce.sh." >&2
    exit 1
  fi
done

# Resolve symlinks and nonexistent path tails without GNU-specific shell utilities.
python3 - "$PWD" "${SPENDCAP_CREDS_DIR:-}" <<'PYTHON'
import os
import sys
repo = os.path.realpath(sys.argv[1])
if sys.argv[2]:
    creds = os.path.realpath(sys.argv[2])
    if os.path.commonpath([repo, creds]) == repo:
        sys.exit("SPENDCAP_CREDS_DIR must be outside the repository (including symlink targets).")
PYTHON

if [ "${WASM+x}" = x ] && [ ! -f "$WASM" ]; then
  echo "Explicit WASM path does not exist or is not a file: $WASM" >&2
  exit 1
fi

echo "== toolchain"
rustc --version
cargo --version
node --version
npm --version
rustup target list --installed | grep -x wasm32-unknown-unknown >/dev/null || rustup target add wasm32-unknown-unknown

echo "== build contract"
if cargo near --version >/dev/null 2>&1; then
  cargo near --version
  (cd contract && cargo near build non-reproducible-wasm --locked)
  export WASM="${WASM:-contract/target/near/quotum_spendcap.wasm}"
else
  echo "cargo-near not found; plain cargo build (install: cargo install cargo-near --version 0.22.0 --locked --no-default-features)"
  (cd contract && RUSTFLAGS="-C link-arg=-s --cfg near" cargo build --target wasm32-unknown-unknown --release --locked)
  export WASM="${WASM:-contract/target/wasm32-unknown-unknown/release/quotum_spendcap.wasm}"
fi
if command -v sha256sum >/dev/null 2>&1; then
  sha256sum "$WASM"
else
  shasum -a 256 "$WASM"
fi

echo "== unit tests"
(cd contract && cargo test --locked --quiet)

echo "== js deps"
npm ci --no-audit --no-fund

echo "== testnet run"
node agent/reproduce.mjs
