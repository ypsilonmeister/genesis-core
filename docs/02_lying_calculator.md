# Lying Calculator — CMP実証実験仕様書

## CMPのTier 1/2自律修復ループを検証するための実験的題材

---

## 0. 目的と位置づけ

本仕様はCognitive Module Protocol (CMP) v0.2の実証実験として設計された **意図的に壊れやすいシステム** である。実用性は不要。観察対象は「CMPの自律修復ループが機能するか」のみ。

**検証する仮説:**
- Tier 1: エラーを検出しAIが自動でモジュール内修復できるか
- Tier 2: 未知の入力パターンに対してAIが新モジュールを自動追加できるか
- §5: モジュール境界の自己検証(把握テスト)が粒度判定に使えるか

---

## 1. システム概要

**計算機としての仕様:**
- 入力: 数式文字列
- 出力: 計算結果(数値)
- 対応演算(v1時点): 四則演算のみ(`+` `-` `*` `/`)
- 例: `"3 + 5 * 2"` → `13`

**v1の意図的な脆弱性:**
- 全角文字に対応しない
- 自然言語表現に対応しない
- 三角関数・べき乗等に対応しない
- 余分な空白・改行・不可視文字に対応しない
- 単位・通貨記号が混入すると壊れる

これらは「バグ」ではなく **攻撃の対象** として意図的に残す。

---

## 2. モジュール構成(創成期 v1)

各モジュールは独立Rustバイナリ。オーケストレータがプロセス管理する。

### Module 1: `normalizer`

```
# CMP Module Charter
What: 入力文字列を正規化して後続モジュールに渡す(空白除去のみ)
Invariants:
  - 入力文字列を破壊的に変更しない(元の入力はログに残す)
  - 空文字列を受け取った場合はエラーを返す
Boundaries:
  - 依存先: なし
  - 被依存先: tokenizer
Extensible: 正規化ルールの追加(全角→半角変換、不可視文字除去等)
Why: 後続モジュールが純粋な解析に集中できるよう、表層的なノイズを除去する
```

**v1実装範囲:** 連続空白を単一空白に圧縮、前後空白をトリム。それ以外は素通し。

---

### Module 2: `tokenizer`

```
# CMP Module Charter
What: 正規化済み文字列を数値・演算子・括弧のトークン列に分解する
Invariants:
  - 認識できないトークンはエラーとして返す(サイレント無視禁止)
  - トークン列の順序は入力順を保持する
Boundaries:
  - 依存先: normalizer
  - 被依存先: parser
Extensible: 認識トークンの種類追加(関数名、定数、単位等)
Why: parserが文法解析に集中できるよう、字句解析を分離する
```

**v1実装範囲:** ASCII数字・小数点・四則演算子(`+ - * /`)・括弧(`( )`)のみ認識。

---

### Module 3: `parser`

```
# CMP Module Charter
What: トークン列を演算子優先度を考慮した抽象構文木(AST)に変換する
Invariants:
  - 演算子優先度: * / は + - より高い
  - 括弧による優先度変更を正しく処理する
  - 不正な文法(演算子連続、括弧不一致等)はエラーを返す
Boundaries:
  - 依存先: tokenizer
  - 被依存先: evaluator
Extensible: 新しい演算子・関数呼び出し構文の追加
Why: evaluatorが純粋な計算に集中できるよう、文法解析を分離する
```

**v1実装範囲:** 四則演算と括弧のみ。再帰下降パーサで実装。

---

### Module 4: `evaluator`

```
# CMP Module Charter
What: ASTを受け取り計算結果(f64)を返す
Invariants:
  - ゼロ除算はエラーを返す(パニック禁止)
  - オーバーフローはエラーを返す(サイレント無視禁止)
  - 計算結果は入力と同一のf64精度で返す
Boundaries:
  - 依存先: parser
  - 被依存先: orchestrator(最終出力)
Extensible: 新しいノードタイプ(関数呼び出し、変数参照等)の評価
Why: 計算ロジックを分離し、parserの変更がevaluatorに波及しないようにする
```

**v1実装範囲:** 加減乗除のみ。f64で計算。

---

## 3. オーケストレータ仕様

### 3.1 基本構成

```
orchestrator (不可侵)
├── モジュールプロセス管理(起動・停止・差替)
├── IPC: Unix Domain Socket + JSON
├── メタデータストア: SQLite (metadata.db)
├── CMPループ制御(エラー検出→修復提案→検証→hot swap)
├── システム憲章 Layer B執行
└── 攻撃AIの呼び出し
```

### 3.2 通信プロトコル

```json
// 要求(orchestrator → module)
{
  "request_id": "uuid",
  "input": "3 + 5 * 2",
  "timestamp": "2026-05-23T12:00:00Z"
}

// 応答(module → orchestrator)
{
  "request_id": "uuid",
  "output": "13",
  "error": null,
  "processing_ms": 12
}

// エラー応答
{
  "request_id": "uuid",
  "output": null,
  "error": {
    "code": "UNKNOWN_TOKEN",
    "message": "Unrecognized character: ３",
    "input_position": 0
  },
  "processing_ms": 3
}
```

### 3.3 エラーコード定義

| コード | 意味 | Tier |
|---|---|---|
| `UNKNOWN_TOKEN` | 認識できない文字・記号 | 1 |
| `SYNTAX_ERROR` | 文法違反 | 1 |
| `DIVISION_BY_ZERO` | ゼロ除算 | 1 |
| `OVERFLOW` | 計算結果がf64範囲外 | 1 |
| `UNKNOWN_PATTERN` | 既存モジュールで処理不能な入力パターン | 2 |
| `MODULE_CRASH` | モジュールプロセスが異常終了 | 1 |

---

## 4. 攻撃AI仕様

### 4.1 役割

Gemini API(推奨: Gemini 2.5 Flash)を使用。オーケストレータから定期的に呼び出され、計算機を壊す入力を生成する。

### 4.2 攻撃パターン(段階的に投入)

**Phase A: 文字レベル攻撃(Week 1から)**
- 全角数字: `"３ + ５ * ２"`
- 全角演算子: `"3 ＋ 5 × 2"`
- 全角括弧: `"（3 + 5）* 2"`
- 不可視文字混入: ゼロ幅スペース等
- 余分な空白・改行: `"3  +\n5 *  2"`

**Phase B: 語彙レベル攻撃(Week 2から)**
- 通貨記号: `"$3 + €5"`
- 単位混入: `"3kg + 5kg"`
- パーセント: `"50% + 30%"`
- 絵文字混入(明示的に検証目的のみ)

**Phase C: 意味レベル攻撃(Week 3から)**
- 自然言語: `"three plus five times two"`
- 日本語: `"三足す五かける二"`
- 混合: `"3 plus 5 * 2"`
- 曖昧表現: `"3くらい + 5"`

**Phase D: 構造レベル攻撃(Week 4から)**
- 未知関数: `"sin(30) + cos(60)"`
- べき乗: `"2^10"` または `"2**10"`
- 対数: `"log(100)"`
- 連鎖計算: `"ans + 5"` (前の結果を参照)

### 4.3 攻撃プロトコル

```
攻撃間隔: 30秒〜3分(ランダム)
攻撃1回あたりの入力数: 1〜5個
攻撃パターン選択: AIが自律的に選択(ただしPhaseは段階的に解放)
```

### 4.4 攻撃AIへのプロンプト

```
あなたは数式パーサーのファジングエージェントです。
以下の計算機システムを壊す入力を生成してください。

システムの現在の状態:
- 対応済みパターン: {成功した入力のサンプル}
- 直近のエラーログ: {直近10件のエラー}

目標: これまでに成功していない新しい失敗パターンを発見する。
同じパターンの繰り返しは避けること。
出力形式: JSON配列 ["入力1", "入力2", ...]
```

### 4.5 攻撃の過適合検出

攻撃AIが「直しやすいパターンばかり選ぶ」過適合を検出するため:

- 攻撃パターンの多様性スコアを計測(直近20攻撃の文字レベル差異)
- 多様性スコアが閾値以下なら、攻撃プロンプトに「これまでと全く異なる攻撃を試みよ」を追加
- 週次でYuyaさんが攻撃ログをレビューし、過適合を人間が判定

---

## 5. CMPループ仕様

### 5.1 Tier 1: モジュール内自動修復

**トリガー:** 同一エラーコードが閾値回数以上発生(デフォルト: 3回)

**修復プロセス:**
1. エラーログ + 対象モジュールのコード + Module Charterを収集
2. Claude APIに修復案を生成させる
3. プロンプト:
   ```
   以下のRustモジュールがエラーを繰り返しています。

   Module Charter: {charter}

   現在のコード: {code}

   エラーログ(直近): {error_log}

   修復案を生成してください。
   制約:
   - Module CharterのInvariantsを破らないこと
   - Module CharterのBoundariesを変更しないこと
   - 修正範囲は最小限にすること

   出力: 修正後のRustコード全体
   ```
4. `cargo build`でコンパイル検証
5. 既存テストスイートを実行
6. 全通過 → hot swap(プロセス入替)
7. 失敗 → 却下、メタデータに転写

**hot swapの手順:**
1. 新バイナリをビルド(`/tmp/modules/{module_name}_new`)
2. 新プロセスを起動、ヘルスチェック
3. IPCを新プロセスに切り替え
4. 旧プロセスをgraceful shutdown
5. 旧バイナリを`/archive/{module_name}_{timestamp}`に退避

### 5.2 Tier 2: モジュール自動追加

**トリガー:** `UNKNOWN_PATTERN`エラーが閾値回数以上発生(デフォルト: 5回)かつ既存モジュール修復で対応不可と判定

**判定プロセス:**
1. Claude APIに「これは既存モジュールの拡張で対応できるか、新モジュールが必要か」を判定させる
2. 新モジュール必要と判定 → 対照群評価(§5.2.1)
3. 対照群より優位な場合のみ新モジュールを生成・追加

**§5.2.1 対照群評価:**
- 案A: AI提案の新モジュール
- 案B: 既存モジュール拡張案(normalizerまたはtokenizerへの追加)
- 判定基準(事前宣言): 案Aが案Bより「エラー率」で統計的有意に優れるか

**新モジュール生成プロセス:**
1. Claude APIに新モジュールのコード + Module Charterを生成させる
2. Module Charterの自動生成を必須とする
3. システム憲章のModule Addition Criteriaを満たすか自動チェック
4. `cargo build` + テスト
5. オーケストレータのモジュール呼び出しチェーンに動的挿入

### 5.3 モジュール呼び出しチェーン

v1の固定チェーン:
```
input → normalizer → tokenizer → parser → evaluator → output
```

Tier 2で新モジュールが追加された場合の例:
```
input → [unicode_normalizer] → normalizer → tokenizer → [natural_lang_parser] → parser → evaluator → output
```

チェーンの変更はオーケストレータが管理する設定ファイル(`chain.toml`)で定義。Tier 2でAIがこのファイルを更新する。

---

## 6. メタデータスキーマ(SQLite)

```sql
-- 全改変の転写ログ
CREATE TABLE modifications (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  timestamp TEXT NOT NULL,
  tier INTEGER NOT NULL,           -- 1 or 2
  module_name TEXT NOT NULL,
  trigger_type TEXT NOT NULL,      -- エラーコード
  trigger_count INTEGER NOT NULL,
  prompt_full TEXT NOT NULL,       -- 生成プロンプト全文
  model_name TEXT NOT NULL,        -- 使用AIモデル
  generated_code TEXT,             -- 生成されたコード
  build_result TEXT NOT NULL,      -- 'success' or 'failure'
  build_error TEXT,
  test_result TEXT,                -- 'pass' or 'fail'
  decision TEXT NOT NULL,          -- 'adopted' or 'rejected'
  rejection_reason TEXT,
  adopted_at TEXT                  -- hot swap完了時刻
);

-- 攻撃ログ
CREATE TABLE attacks (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  timestamp TEXT NOT NULL,
  attacker_model TEXT NOT NULL,
  inputs TEXT NOT NULL,            -- JSON配列
  phase TEXT NOT NULL,             -- A/B/C/D
  diversity_score REAL,
  results TEXT NOT NULL            -- JSON: 各inputのエラーコードまたは成功
);

-- モジュール状態ログ
CREATE TABLE module_snapshots (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  timestamp TEXT NOT NULL,
  module_name TEXT NOT NULL,
  version INTEGER NOT NULL,        -- hot swapごとにインクリメント
  code TEXT NOT NULL,
  charter TEXT NOT NULL,
  modification_id INTEGER,         -- modifications.idへの参照
  FOREIGN KEY (modification_id) REFERENCES modifications(id)
);

-- 把握テスト結果(§5のComprehension Test)
CREATE TABLE comprehension_tests (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  timestamp TEXT NOT NULL,
  module_name TEXT NOT NULL,
  judge_model TEXT NOT NULL,
  generated_summary TEXT NOT NULL,  -- LLMが生成した1文要約
  charter_what TEXT NOT NULL,       -- 個別憲章のWhat
  match_result TEXT NOT NULL,       -- 'match' or 'mismatch'
  split_candidate INTEGER NOT NULL  -- 1=分割候補, 0=不要
);
```

---

## 7. システム憲章(v1)

Layer A (`charter/system.md`) と Layer B (`charter/enforcement.rs`) として独立ファイルで保持する。本ドキュメントから物理的に切り出し、AIの改変対象外とする。詳細はリポジトリの `charter/` ディレクトリを参照。

---

## 8. 評価指標(30日間観察)

| 指標 | 測定方法 | 目標値(仮) |
|---|---|---|
| Tier 1成功率 | 採用された修復 / 試みた修復 | > 60% |
| Tier 2発動回数 | 新モジュール追加回数 | 記録のみ |
| hot swap無停止率 | 成功したhot swap / 試みた | > 90% |
| 把握テスト一致率 | match / 全テスト数 | > 70% |
| 攻撃多様性スコア | 直近20攻撃の平均文字差異 | > 0.5 |
| 人間介入回数 | Tier 3発動 + 障害通知 | 記録のみ |
| Rustビルド失敗率 | ビルド失敗 / 全修復試行 | 記録のみ |
| 憲章違反検出回数 | Layer B違反 | 0が理想 |

**観察の核心:** 攻撃AIがPhase Dまで到達したとき、Aにも対応できていたら「汎化」。Phase Aのパターンしか直せなくなっていたら「過適合」。この判定が本実験の結論になる。

---

## 9. ディレクトリ構成

```
genesis-core/
├── Cargo.toml                 # workspace定義
├── orchestrator/
│   ├── src/main.rs
│   ├── src/cmp_loop.rs        # Tier 1/2ループ
│   ├── src/charter.rs         # Layer B執行
│   ├── src/metadata.rs        # SQLite書き込み
│   ├── src/hot_swap.rs        # プロセス入替
│   └── src/attacker.rs        # 攻撃AI呼び出し
├── modules/
│   ├── normalizer/src/main.rs
│   ├── tokenizer/src/main.rs
│   ├── parser/src/main.rs
│   └── evaluator/src/main.rs
├── charter/
│   ├── system.md              # Layer A(AI書き換え禁止)
│   └── enforcement.rs         # Layer B(AI書き換え禁止)
├── chain.toml                 # モジュール呼び出しチェーン定義
├── metadata.db                # SQLite(gitignore)
└── archive/                   # 旧バイナリ退避場所
```

---

## 10. 実装順序(Claude Codeへの推奨)

1. **Cargo workspaceと4モジュールの骨格**
   - 各モジュールが起動してIPCで疎通できる状態まで
   - `"3 + 5 * 2"` → `13` が通る
2. **オーケストレータの基本ループ**
   - モジュールプロセス管理
   - エラー検出とログ記録
   - SQLiteへのメタデータ転写
3. **攻撃AIの最小実装**
   - Phase Aのみ
   - 攻撃ログのSQLite記録
4. **Tier 1修復ループ**
   - Claude API呼び出し
   - `cargo build`検証
   - hot swap
5. **把握テスト(§5)**
   - 定期実行
   - 結果のSQLite記録
6. **Tier 2追加ループ**
   - 対照群評価込み
7. **攻撃Phase B/C/D の解放**

---

## 11. Claude Codeへの補足指示

- Rustエディションは2021
- 非同期ランタイムは`tokio`
- IPCは`tokio::net::UnixListener`と`UnixStream`
- JSONシリアライズは`serde_json`
- SQLiteは`rusqlite`
- APIコールは`reqwest`(async)
- ビルドは`tokio::process::Command`でサブプロセス実行
- エラー型は`thiserror`で定義
- ログは`tracing`クレート

**最初のコミットで動かすべきもの:** `"3 + 5 * 2"` → `13` が通ること。それだけ。
攻撃AIもCMPループも後回し。まず4モジュールがIPCで繋がること。
