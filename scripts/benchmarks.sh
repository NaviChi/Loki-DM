#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

URL_10GB="${URL_10GB:-https://proof.ovh.net/files/1Gb.dat}"
URL_SMALL="${URL_SMALL:-https://proof.ovh.net/files/1Mb.dat}"
OUT_DIR="${OUT_DIR:-$ROOT/.bench-out}"
SMALL_PARALLEL="${SMALL_PARALLEL:-8}"
mkdir -p "$OUT_DIR"

echo "Building release CLI once..."
cargo build --release -p loki-dm-cli
BIN="${ROOT}/target/release/loki-dm"

echo "[1/4] Loki DM single large file (${URL_10GB})"
time "$BIN" \
  download "$URL_10GB" --output "$OUT_DIR/loki-10gb.bin" --connections 12 --overwrite

echo "[2/4] curl baseline"
time curl -L "$URL_10GB" -o "$OUT_DIR/curl-10gb.bin"

echo "[3/4] aria2 baseline"
if command -v aria2c >/dev/null 2>&1; then
  time aria2c -x16 -s16 -o aria2-10gb.bin -d "$OUT_DIR" "$URL_10GB"
else
  echo "aria2c not found; skipping aria2 baseline"
fi

echo "[4/4] 100 small files stress (parallel=${SMALL_PARALLEL})"
time seq 1 100 | xargs -P "${SMALL_PARALLEL}" -I{} sh -c \
  '"$0" download "$1?i=$2" --output "$3/small-$2.bin" --connections 4 --overwrite >/dev/null' \
  "$BIN" "$URL_SMALL" "{}" "$OUT_DIR"

echo "Benchmarks complete. Outputs in: $OUT_DIR"
