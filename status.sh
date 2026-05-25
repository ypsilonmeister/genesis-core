#!/bin/bash

DB_PATH="./metadata.db"

if [ ! -f "$DB_PATH" ]; then
    echo "Error: $DB_PATH not found."
    exit 1
fi

echo "=== Genesis Core System Status ==="
echo "Date: $(date)"
echo "----------------------------------"

# 最新の攻撃ログ
echo "[Latest Attacks]"
sqlite3 "$DB_PATH" "SELECT timestamp, phase, diversity_score FROM attacks ORDER BY id DESC LIMIT 5;" | column -t -s '|'
echo ""

# 最近の修復ログ
echo "[Latest Modifications]"
sqlite3 "$DB_PATH" "SELECT timestamp, module_name, tier, build_result FROM modifications ORDER BY id DESC LIMIT 5;" | column -t -s '|'
echo ""

# 統計情報の簡易サマリ
echo "[Summary Statistics]"
echo -n "Total Attacks: "
sqlite3 "$DB_PATH" "SELECT count(*) FROM attacks;"
echo -n "Total Modifications: "
sqlite3 "$DB_PATH" "SELECT count(*) FROM modifications;"
echo "----------------------------------"
