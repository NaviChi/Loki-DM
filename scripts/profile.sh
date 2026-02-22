#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

URL="${URL:-https://proof.ovh.net/files/1Gb.dat}"

echo "Generating flamegraph"
cargo flamegraph -p loki-dm-cli -- download "$URL" --output ./profile.bin --overwrite

echo "Generating samply profile"
samply record target/release/loki-dm download "$URL" --output ./profile.bin --overwrite
