# 改変提案: 論文グレード・クリーンラン準備（データ品質 + 最小シード）

- 状態: Draft / 人間承認待ち
- 対象: `orchestrator/**`（**不可侵領域 = 人間のみ編集**, Tier 3）+ `modules/**`（シード作り直し, Tier 1/2）
- 目的: orchestrator を固定独立変数にし、modules 進化だけをクリーンに観測する再実験のための足場づくり
- 背景: 現 run(5/28–5/31)は観測中に orchestrator を4回改変しており**交絡**。論文の主張を支える因果が切れている。現データは `archive/metadata_run1_20260531.db` に保全済み。
- Layer B 自己検証: 以下はすべて DB への追記・関数配線・テキスト抽出修正のみ。HI-1〜HI-5 のいずれにも抵触しない。

---

## Fix A — `module_snapshots` を配線する（コード系統樹／監査証跡）

**事実**: `metadata.rs:190 insert_module_snapshot()` は `#[allow(dead_code)]` 付きで定義されているが、orchestrator 内に**呼び出しが1つも無い**（grep 0件）。よって `module_snapshots` は0件。各モジュールのバージョン履歴（系統樹）が一切残っていない。

**修正**: `cmp_loop.rs` の採用3経路すべてで、`insert_modification()` が返す `modification_id` を使って直後に `insert_module_snapshot()` を呼ぶ。
- Tier 1 修復採用（約 line 234 付近）
- Tier 2 拡張採用（約 line 560 付近 `Tier2Outcome::Extended`）
- Tier 2 新規モジュール採用（約 line 700 付近）

引数: `module_name`, `version = (現 max(version) + 1)`, `code = 採用したmain.rs全文`, `charter = 抽出したCharterコメント`, `modification_id = Some(id)`。
version 採番のため `metadata.rs` に `fn next_snapshot_version(&self, module: &str) -> Result<i64>`（`SELECT COALESCE(MAX(version),0)+1 ...`、SELECT は破壊的でなく HI-3 非該当）を追加。

→ これで「種→現在」の全リビジョンが diff 可能になり、論文の Figure（モジュール成長曲線・系統樹）が作れる。

## Fix B — チャーン計測を可能にする（論文の中核指標）

**事実**: `modifications` テーブルは `trigger_type` / `trigger_count` は持つが、**実際に発火した入力文字列を保存していない**（`inputs` 列があるのは `attacks` テーブルのみ）。このため「採用214回」を *distinct な能力獲得* と *同一バグの再修復（チャーン）* に分類できない＝収束曲線が描けない。

**修正**:
1. スキーマに列追加: `ALTER TABLE modifications ADD COLUMN trigger_inputs TEXT`（新規DBでは CREATE 文に直接追加。`ALTER` は HI-3 の破壊的動詞だが、これは*人間が*スキーマ初期化時に行う DDL であり、ランタイムの RepairAi 経路ではない。新規DB運用なので実際には CREATE TABLE に1行足すだけで ALTER 不要）。
2. `ModificationRecord` に `pub trigger_inputs: Option<String>`（発火させた未知入力の JSON 配列）を追加し、`insert_modification` の INSERT に含める。
3. Tier 発火時点で握っている未知パターン入力群を `ModificationRecord` に詰める。

→ これで「同じ入力が何度修復対象になったか」「採用後にその入力が再発したか（回帰）」が SQL で出せ、収束/非収束を定量化できる。

## Fix C — 把握テスト指標の汚染を除く

**事実**: `comprehension_tests` の mismatch 13件のうち複数が `charter_what = "(no What section found)"`。これは**Charter の "What:" ブロック抽出が一部フォーマットで失敗している**ことを意味し、意味ドリフトではなく**パースバグ由来の偽 mismatch**。現状の「95% match」はこの汚染を含む。

**修正**:
1. What 抽出ロジックを堅牢化（`// What:` 行の後続インデント行を末尾まで取得、`====` バナー区切りの新旧2形式に対応）。テストケースとして既存4モジュールの Charter で `(no What section found)` が出ないことを保証。
2. **別件として要切り分け**: `generated_summary` の日本語が文字化けして見える件は、(a) 端末表示のみの問題か (b) DB保存が壊れているか未確定。再実験前に生バイト（`hex(generated_summary)`）を確認し、保存破損なら AI 応答取り込み経路（Windows の stdout/JSON エンコーディング）の UTF-8 固定を別提案として出す。指標として公表するなら先に確定が必須。

## シード設計 — compat ベースの最小モジュール

**事実**: Week 1 シード(`d449416`)は `use tokio::net::UnixListener;` 直書きで、**今の Windows ではビルド不能**（新規モジュール失敗と同根）。「最初のコミットに戻す」だけでは走らない。

**修正**: Week 1 相当の最小能力を持つクリーンシードを作る。各モジュールは:
- import を `use compat::UnixListener;` に
- Cargo.toml を workspace-deps テンプレート（`new_module_addition_fix.md` と同一）に
- Charter コメントは Week 1 の日本語をそのまま保持
- 能力は最小: normalizer=全角/半角+空白正規化、tokenizer=数値と `+ - * / ( )`、parser=7トークンAST、evaluator=`+ - * /`
- `"3 + 5 * 2" → 13` が通ることだけを受け入れ条件に

これをリセット点とし、`modules/*/src` と `modules/*/Cargo.toml` を置換。**orchestrator と compat は現 HEAD 固定**。

## 再実験プロトコル（論文用）

1. 新規・空の `metadata.db`（旧は archive 済み）。
2. 開始時の git commit hash を記録（orchestrator 固定の証跡）。
3. 攻撃 AI に駆動させ、**停止条件を事前に定義**（例: 実時間 X 時間 or 改変 M 件）。
4. 取得物: snapshots（系統樹）, trigger_inputs（チャーン）, クリーンな comprehension。
5. 期待する観測: (a) 新規モジュール生成が**初めて成功**するか, (b) 収束 or 非収束, (c) 修復≫創造の非対称が固定 orchestrator でも再現するか。

## 付随クリーンアップ（任意）

`archive/` に過去 run(5/25–5/27)のバイナリスナップショットが 34MB級 × 約45個（計 ~1.5GB、`math_expander` 等の旧モジュール含む）。再実験前に別ディレクトリへ退避 or 削除を検討（gitignore済みなのでリポジトリには影響なし）。
