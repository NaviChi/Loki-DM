#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

TARGETS=(
  "${TARGET_LINUX:-x86_64-unknown-linux-gnu}"
  "${TARGET_WINDOWS:-x86_64-pc-windows-gnu}"
  "${TARGET_MACOS_INTEL:-x86_64-apple-darwin}"
  "${TARGET_MACOS_ARM:-aarch64-apple-darwin}"
)

HOST_TARGET="$(rustc -vV | awk '/host:/ { print $2 }')"
RUSTC_BIN="${RUSTC:-$(rustup which rustc 2>/dev/null || command -v rustc)}"
FAILED=()
SKIPPED=()

setup_linux_cross_cc() {
  local target="$1"
  if [[ "$target" != "x86_64-unknown-linux-gnu" ]]; then
    return 0
  fi

  if [[ "$HOST_TARGET" == "x86_64-unknown-linux-gnu" ]]; then
    return 0
  fi

  if command -v x86_64-linux-gnu-gcc >/dev/null 2>&1; then
    export CC_x86_64_unknown_linux_gnu="x86_64-linux-gnu-gcc"
    return 0
  fi

  if command -v zig >/dev/null 2>&1; then
    local tool_dir="$ROOT/.artifacts/toolchain"
    mkdir -p "$tool_dir"
    local cc_wrapper="$tool_dir/x86_64-linux-gnu-gcc"
    cat >"$cc_wrapper" <<'WRAP'
#!/usr/bin/env bash
args=()
for arg in "$@"; do
  if [[ "$arg" == "--target=x86_64-unknown-linux-gnu" ]]; then
    args+=("-target" "x86_64-linux-gnu")
  else
    args+=("$arg")
  fi
done
exec zig cc "${args[@]}"
WRAP
    chmod +x "$cc_wrapper"
    export CC_x86_64_unknown_linux_gnu="$cc_wrapper"
    return 0
  fi

  return 1
}

for target in "${TARGETS[@]}"; do
  echo "==> Checking target: $target"
  rustup target add "$target" || true

  unset CC_x86_64_unknown_linux_gnu
  if ! setup_linux_cross_cc "$target"; then
    SKIPPED+=("$target (missing cross C compiler; install x86_64-linux-gnu-gcc or zig)")
    continue
  fi

  if ! RUSTC="$RUSTC_BIN" cargo check --target "$target" -p loki-dm-core -p loki-dm-cli; then
    FAILED+=("$target (core/cli)")
    continue
  fi

  if [[ "${CHECK_GUI_CROSS:-0}" == "1" || "$target" == "$HOST_TARGET" ]]; then
    if ! RUSTC="$RUSTC_BIN" cargo check --target "$target" -p loki-dm-gui; then
      FAILED+=("$target (gui)")
    fi
  fi
done

if ((${#FAILED[@]} > 0)); then
  echo "Cross-check failures:"
  printf '  - %s\n' "${FAILED[@]}"
  if ((${#SKIPPED[@]} > 0)); then
    echo "Skipped targets:"
    printf '  - %s\n' "${SKIPPED[@]}"
  fi
  exit 1
fi

if ((${#SKIPPED[@]} > 0)); then
  echo "Skipped targets:"
  printf '  - %s\n' "${SKIPPED[@]}"
fi

echo "Cross checks completed successfully."
