#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"

is_truthy() {
  local raw="${1:-}"
  local trimmed
  trimmed="$(printf '%s' "$raw" | tr '[:upper:]' '[:lower:]' | xargs)"
  [[ "$trimmed" == "1" || "$trimmed" == "true" ]]
}

resolve_file() {
  local candidate
  for candidate in "$@"; do
    if [[ -n "$candidate" && -f "$candidate" ]]; then
      printf '%s\n' "$candidate"
      return 0
    fi
  done
  return 1
}

exe_suffix() {
  case "$(uname -s 2>/dev/null || echo unknown)" in
    MINGW*|MSYS*|CYGWIN*|Windows_NT) printf '.exe' ;;
    *) printf '' ;;
  esac
}

show_usage() {
  cat <<'EOF'
Usage: ./rust/cutover.sh <status|verify|switch <rust|legacy>|help> [--apply]

Commands:
  status               Inspect active mode and Rust artifacts across all 5 surfaces
  verify               Run non-destructive surface cutover and test checks
  switch <rust|legacy> Print/apply kill-switch settings (add --apply to stage workers)
  help                 Show this help
EOF
}

show_status() {
  local ext
  ext="$(exe_suffix)"

  local cli_mode="rust"
  if is_truthy "${LEGIONCODE_LEGACY_NODE:-}"; then
    cli_mode="legacy (LEGIONCODE_LEGACY_NODE=${LEGIONCODE_LEGACY_NODE})"
  fi
  local cli_bin="missing"
  cli_bin="$(resolve_file "${LEGIONCODE_RUST_BIN:-}" "$SCRIPT_DIR/dist/bin/legioncode${ext}" "$SCRIPT_DIR/target/release/legioncode${ext}")" || cli_bin="missing"
  local dist_exe="missing"
  dist_exe="$(resolve_file "$REPO_ROOT/apps/server/dist-exe/legioncode${ext}")" || dist_exe="missing"
  local rust_web="missing"
  rust_web="$(resolve_file "$SCRIPT_DIR/dist/web-app/index.html")" || rust_web="missing"

  local desktop_mode="rust"
  if is_truthy "${LEGIONCODE_LEGACY_ELECTRON:-}"; then
    desktop_mode="legacy (LEGIONCODE_LEGACY_ELECTRON=${LEGIONCODE_LEGACY_ELECTRON})"
  fi
  local desktop_bin="missing"
  desktop_bin="$(resolve_file "$SCRIPT_DIR/dist/desktop/legioncode-desktop${ext}" "$SCRIPT_DIR/target/release/legioncode-desktop${ext}")" || desktop_bin="missing"

  local web_mode="rust"
  if is_truthy "${LEGIONCODE_LEGACY_WEB:-}"; then
    web_mode="legacy (LEGIONCODE_LEGACY_WEB=${LEGIONCODE_LEGACY_WEB})"
  fi
  local wasm_node="missing"
  wasm_node="$(resolve_file "$SCRIPT_DIR/dist/wasm-nodejs/legion_wasm_ffi.js")" || wasm_node="missing"
  local wasm_web="missing"
  wasm_web="$(resolve_file "$SCRIPT_DIR/dist/wasm-web/legion_wasm_ffi.js")" || wasm_web="missing"

  local mobile_script="pending"
  if [[ -f "$REPO_ROOT/apps/mobile/scripts/sync-rust-mobile-ffi.mjs" ]]; then
    mobile_script="present"
  fi
  local mobile_ready="complete (5/5)"
  local rel
  for rel in \
    "dist/mobile/include/legion_mobile_ffi.h" \
    "dist/mobile/swift/LegionMobileCore.swift" \
    "dist/mobile/kotlin/LegionMobileCore.kt" \
    "dist/mobile/android/LegionMobileBridge.kt" \
    "dist/mobile/apple/Package.swift"; do
    if [[ ! -f "$SCRIPT_DIR/$rel" ]]; then
      mobile_ready="incomplete"
      break
    fi
  done

  local worker_mode="rust"
  if is_truthy "${LEGIONCODE_LEGACY_WORKER:-}"; then
    worker_mode="legacy (LEGIONCODE_LEGACY_WORKER=${LEGIONCODE_LEGACY_WORKER})"
  fi
  local worker_ready="complete (5/5)"
  for rel in \
    "dist/cloudflare-workers/worker.mjs" \
    "dist/cloudflare-workers/worker_runtime.mjs" \
    "dist/cloudflare-workers/wrangler.toml" \
    "dist/cloudflare-workers/legion_wasm_ffi.js" \
    "dist/cloudflare-workers/legion_wasm_ffi_bg.wasm"; do
    if [[ ! -f "$SCRIPT_DIR/$rel" ]]; then
      worker_ready="incomplete"
      break
    fi
  done
  local relay_staged="no (run stage:rust)"
  [[ -f "$REPO_ROOT/infra/relay/dist/rust-worker/worker.mjs" ]] && relay_staged="yes"
  local task_cache_staged="no (run stage:rust)"
  [[ -f "$REPO_ROOT/infra/task-cache/dist/rust-worker/worker.mjs" ]] && task_cache_staged="yes"

  printf 'LegionCode Rust Cutover Status (%s)\n' "$REPO_ROOT"
  printf '%s\n' '------------------------------------------------------------'
  printf 'CLI / Server       : %s\n' "$cli_mode"
  printf '  Rust binary      : %s\n' "$cli_bin"
  printf '  dist-exe staged  : %s\n' "$dist_exe"
  printf '  dist/web-app     : %s\n' "$rust_web"
  printf 'Desktop            : %s\n' "$desktop_mode"
  printf '  Rust binary      : %s\n' "$desktop_bin"
  printf 'Web WASM           : %s\n' "$web_mode"
  printf '  wasm-nodejs      : %s\n' "$wasm_node"
  printf '  wasm-web         : %s\n' "$wasm_web"
  printf 'Mobile FFI         : sync:rust-ffi (script: %s)\n' "$mobile_script"
  printf '  dist/mobile      : %s\n' "$mobile_ready"
  printf 'Cloudflare Workers : %s\n' "$worker_mode"
  printf '  dist/workers     : %s\n' "$worker_ready"
  printf '  relay staged     : %s\n' "$relay_staged"
  printf '  task-cache staged: %s\n' "$task_cache_staged"
}

run_verify() {
  local ext
  ext="$(exe_suffix)"
  cd "$REPO_ROOT"

  local cli_bin=""
  if cli_bin="$(resolve_file "${LEGIONCODE_RUST_BIN:-}" "$SCRIPT_DIR/dist/bin/legioncode${ext}" "$SCRIPT_DIR/target/release/legioncode${ext}")"; then
    echo "[verify] Rust CLI --version..."
    "$cli_bin" --version
  else
    echo "[verify] Rust CLI binary missing; skipping --version check."
  fi

  local -a test_files=(
    "apps/server/src/rustCutover.test.ts"
    "scripts/build-cli-archive.test.ts"
  )
  if [[ -f "$REPO_ROOT/packages/client-runtime/src/wasm/rustCoreBridge.test.ts" ]]; then
    test_files+=("packages/client-runtime/src/wasm/rustCoreBridge.test.ts")
  fi
  echo "[verify] Vitest cutover tests..."
  vp test run "${test_files[@]}"

  local desktop_bin=""
  if desktop_bin="$(resolve_file "$SCRIPT_DIR/dist/desktop/legioncode-desktop${ext}" "$SCRIPT_DIR/target/release/legioncode-desktop${ext}")" \
    && [[ -f "$REPO_ROOT/apps/desktop/scripts/start-electron.mjs" ]]; then
    echo "[verify] Desktop Rust --self-check..."
    node "$REPO_ROOT/apps/desktop/scripts/start-electron.mjs" --self-check
  fi

  if [[ -f "$REPO_ROOT/apps/mobile/scripts/sync-rust-mobile-ffi.mjs" ]]; then
    echo "[verify] Mobile FFI --check..."
    node "$REPO_ROOT/apps/mobile/scripts/sync-rust-mobile-ffi.mjs" --check
  fi

  local pkg
  for pkg in relay task-cache; do
    if [[ -f "$REPO_ROOT/infra/$pkg/scripts/stage-rust-worker.mjs" ]]; then
      echo "[verify] $pkg worker --check..."
      node "$REPO_ROOT/infra/$pkg/scripts/stage-rust-worker.mjs" --check
    fi
  done

  echo "[verify] All cutover checks passed."
}

run_switch() {
  local mode="${1:-}"
  local apply="${2:-false}"
  local normalized
  normalized="$(printf '%s' "$mode" | tr '[:upper:]' '[:lower:]' | xargs)"
  if [[ "$normalized" != "rust" && "$normalized" != "legacy" ]]; then
    echo "Unknown switch mode '$mode'. Expected 'rust' or 'legacy'." >&2
    return 1
  fi

  local -a vars=(
    "LEGIONCODE_LEGACY_NODE"
    "LEGIONCODE_LEGACY_ELECTRON"
    "LEGIONCODE_LEGACY_WEB"
    "LEGIONCODE_LEGACY_WORKER"
  )

  if [[ "$normalized" == "rust" ]]; then
    local name
    for name in "${vars[@]}"; do
      unset "$name"
    done
    echo "Switched current shell environment to RUST cutover mode."
    echo "To apply in your shell:"
    for name in "${vars[@]}"; do
      echo "  unset $name"
    done
    echo "Mobile FFI sync: pnpm --filter @legioncode/mobile sync:rust-ffi"
    echo "Worker staging : pnpm --filter legioncode-relay stage:rust && pnpm --filter @legioncode/task-cache stage:rust"

    if [[ "$apply" == "true" ]]; then
      cd "$REPO_ROOT"
      local pkg
      for pkg in relay task-cache; do
        if [[ -f "$REPO_ROOT/infra/$pkg/scripts/stage-rust-worker.mjs" ]]; then
          echo "[switch] Staging $pkg Rust worker..."
          node "$REPO_ROOT/infra/$pkg/scripts/stage-rust-worker.mjs"
        fi
      done
    fi
  else
    local name
    for name in "${vars[@]}"; do
      export "$name=1"
    done
    echo "Switched current shell environment to LEGACY rollback mode."
    echo "To apply in your shell:"
    for name in "${vars[@]}"; do
      echo "  export $name=1"
    done
  fi
}

cmd="${1:-}"
arg2="${2:-}"
arg3="${3:-}"
apply_flag="false"
if [[ "$arg2" == "--apply" || "$arg3" == "--apply" ]]; then
  apply_flag="true"
  if [[ "$arg2" == "--apply" ]]; then
    arg2=""
  fi
fi

if [[ -z "$cmd" ]]; then
  if [[ ! -t 0 ]]; then
    show_status
    exit 0
  fi
  echo "LegionCode Cutover Controller"
  echo "  1) status"
  echo "  2) verify"
  echo "  3) switch rust"
  echo "  4) switch legacy"
  echo "  5) exit"
  read -r -p "Select action [1-5]: " choice
  case "${choice// /}" in
    1) cmd="status" ;;
    2) cmd="verify" ;;
    3) cmd="switch"; arg2="rust" ;;
    4) cmd="switch"; arg2="legacy" ;;
    5) exit 0 ;;
    *) echo "Invalid selection '$choice'." >&2; exit 1 ;;
  esac
fi

case "$(printf '%s' "$cmd" | tr '[:upper:]' '[:lower:]')" in
  status) show_status ;;
  verify) run_verify ;;
  switch)
    if [[ -z "$arg2" ]]; then
      read -r -p "Target mode (rust|legacy): " arg2
    fi
    run_switch "$arg2" "$apply_flag"
    ;;
  help|-h|--help) show_usage ;;
  *) echo "Unknown command '$cmd'. Run with 'help' for usage." >&2; exit 1 ;;
esac
