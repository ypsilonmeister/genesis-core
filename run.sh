#!/usr/bin/env bash
# =============================================================================
# run.sh — 30 日連続運転スクリプト
#
# 使い方:
#   ./run.sh             # フォアグラウンドで起動
#   ./run.sh &           # バックグラウンドで起動
#   nohup ./run.sh &     # ログアウト後も継続
#
# 前提:
#   - cargo build --workspace で全バイナリが target/debug/ に生成済み
#   - claude CLI または Gemini CLI が PATH 上に存在
#   - .env を編集して API キー等を設定済み (CLI を使う場合は不要)
#
# ログ出力: RUST_LOG 環境変数で制御 (デフォルト: info,orchestrator=debug)
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# .env が存在すれば読み込む
if [ -f ".env" ]; then
    # shellcheck disable=SC1091
    set -a
    source ".env"
    set +a
fi

# デフォルト環境変数 (未設定時のみ適用)
export REPAIR_BACKEND="${REPAIR_BACKEND:-${CLAUDE_BACKEND:-claude}}"
export ATTACK_BACKEND="${ATTACK_BACKEND:-${GEMINI_BACKEND:-gemini}}"
export ATTACK_PHASE="${ATTACK_PHASE:-D}"
export ATTACK_INTERVAL_MIN_SECS="${ATTACK_INTERVAL_MIN_SECS:-30}"
export ATTACK_INTERVAL_MAX_SECS="${ATTACK_INTERVAL_MAX_SECS:-180}"
export TIER1_TRIGGER_COUNT="${TIER1_TRIGGER_COUNT:-3}"
export TIER2_TRIGGER_COUNT="${TIER2_TRIGGER_COUNT:-5}"
export RUST_LOG="${RUST_LOG:-info,orchestrator=debug}"

# UDS ソケットディレクトリを作成
mkdir -p /tmp/genesis-core

echo "[run.sh] genesis-core 30-day run started at $(date -Iseconds)"
echo "[run.sh] ATTACK_PHASE=${ATTACK_PHASE}, REPAIR_BACKEND=${REPAIR_BACKEND}, ATTACK_BACKEND=${ATTACK_BACKEND}"

# ビルドが必要か確認
if [ ! -f "target/debug/orchestrator" ]; then
    echo "[run.sh] orchestrator binary not found, building..."
    cargo build --workspace
fi

exec cargo run -p orchestrator
