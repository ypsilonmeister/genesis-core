# 改変提案: Tier 2 新規モジュール追加が全滅する原因と修正

- 状態: 承認済み・適用済み（人間承認 2026-05-31、`cmp_loop.rs` に反映）
- 対象ファイル: `orchestrator/src/cmp_loop.rs`（**不可侵領域 = 人間のみ編集**）
- Tier 判定: orchestrator の変更のため **Tier 3（人間承認必須）**
- 作成者: Repair AI (Claude)
- Layer B 自己検証: 本提案はプロンプト文字列リテラルの変更のみ。HI-1（誤答）/ HI-2（直接通信）/ HI-3（メタデータ破壊）/ HI-4（攻撃AIのコードアクセス）/ HI-5（charter書込）のいずれにも抵触しない。

---

## 1. 事実（metadata.db `modifications` より）

新規モジュール追加（Tier 2 の "新モジュール" パス）は **試行 7 件すべて却下**。内訳:

| id | module | 失敗原因（build_error） |
|---|---|---|
| 103 | special_math | `tokio::net::{UnixListener, UnixStream}` 解決不可 (E0432) |
| 110 | complex_evaluator | `tokio::io::{AsyncReadExt, AsyncWriteExt}` 解決不可（`io-util` feature 無効）|
| 113 | handler | `#[tokio::main]` に `rt`/`rt-multi-thread` 必要 + 上記 io-util |
| 115 | complexifier | `#[tokio::main]` に `rt-multi-thread` 必要 + `tokio::net::Unix*` 解決不可 |
| 116 | complex_calculator | `Chars: ExactSizeIterator` 未実装（`.chars().enumerate().rev()`）= **純粋なロジックバグ** |
| 117 | complex_evaluator | `tokio::net::{UnixListener, UnixStream}` 解決不可 (E0432) |
| 148 | scientific_resolver | `tokio::net::UnixStream` 解決不可 (E0432) |

**7 件中 6 件が同一の系統的原因**（Windows 非互換 + tokio feature 不足）。
残り 1 件（116）のみ通常のロジックバグで、これは修復ループの通常範囲。

## 2. 根本原因

新規モジュール生成プロンプト（`cmp_loop.rs` 約 571–589 行）が **本プロジェクト固有の規約を AI に伝えていない**。

1. **`compat` クレート規約が未伝達。**
   既存モジュールは Windows で動かすため `use compat::UnixListener;`（`compat::UnixStream`）を使う。
   `compat` は `#[cfg(unix)]` で `tokio::net::Unix*` を、`#[cfg(not(unix))]` で TCP 実装を re-export する shim（`compat/src/lib.rs`）。
   プロンプトは「modules/normalizer と同じ通信プロトコルを使え」と*パス名だけ*を渡すが、AI はファイルを読めないため教科書どおり `tokio::net::UnixListener` を書き、Windows で `#[cfg(unix)]` ゲートに弾かれる。

2. **tokio の feature 不足。**
   プロンプトは「Cargo.toml は最小限に」と指示するため、AI は `tokio = { version = "1", features = [...] }` を独自生成し、`rt-multi-thread` / `io-util` / `net` が欠落する。
   既存モジュールは `tokio = { workspace = true }`（workspace 側で `features = ["full"]`）を使うことでこれを回避している。

つまり**プロンプト／雛形の欠陥であり、CMP ループ自体や AI 能力の問題ではない**。実際 Tier 1/2 の*既存モジュール拡張*は 279/287 が採用されており、新規追加だけが規約欠落で全滅している。

## 3. 修正案（`cmp_loop.rs` の `new_mod_prompt`）

`Requirements:` ブロックに以下を追加し、確実に動く雛形をプロンプトへ埋め込む。

```diff
                 Requirements:\n\
-                - Module must receive and return JSON over UDS (Unix Domain Socket)\n\
-                - Use the same communication protocol as existing modules (modules/normalizer)\n\
+                - Module must receive and return JSON over the socket\n\
+                - CRITICAL (cross-platform): import the listener/stream from the `compat` crate, NOT from tokio.\n\
+                  Use `use compat::UnixListener;` and (if connecting downstream) `use compat::UnixStream;`.\n\
+                  NEVER write `use tokio::net::UnixListener` / `UnixStream` — it is `#[cfg(unix)]`-gated and fails to build on Windows.\n\
                 - Include CMP Module Charter comment at the top\n\
-                - Keep code and Cargo.toml extremely concise and minimal to prevent token overflow.\n\
-                - Limit Cargo.toml dependencies to only necessary ones (tokio, serde, serde_json, thiserror, anyhow, tracing). Do not include unnecessary packages.\n\n\
+                - Keep main.rs concise to prevent token overflow.\n\
+                - Cargo.toml MUST use workspace dependencies exactly as below (do NOT pin versions or features yourself; tokio's features come from the workspace as `full`). Use this template verbatim, replacing only the package/bin name:\n\
+                  ```\n\
+                  [package]\n\
+                  name = \"<module_name>\"\n\
+                  version = \"0.1.0\"\n\
+                  edition.workspace = true\n\
+                  rust-version.workspace = true\n\
+                  license.workspace = true\n\
+                  authors.workspace = true\n\
+                  repository.workspace = true\n\
+                  publish.workspace = true\n\
+                  [[bin]]\n\
+                  name = \"<module_name>\"\n\
+                  path = \"src/main.rs\"\n\
+                  [dependencies]\n\
+                  tokio = { workspace = true }\n\
+                  serde = { workspace = true }\n\
+                  serde_json = { workspace = true }\n\
+                  thiserror = { workspace = true }\n\
+                  anyhow = { workspace = true }\n\
+                  tracing = { workspace = true }\n\
+                  tracing-subscriber = { workspace = true }\n\
+                  uuid = { workspace = true }\n\
+                  chrono = { workspace = true }\n\
+                  socket2 = { workspace = true }\n\
+                  compat = { workspace = true }\n\
+                  ```\n\n\
```

### 補足（任意だが推奨）
- 上記テンプレートをハードコードで埋め込む代わりに、生成直前に `executor` で `modules/normalizer/Cargo.toml` と `modules/normalizer/src/main.rs` を読み込み、プロンプトへ「参照実装」として丸ごと注入する方式の方が、将来の規約変更に追従できて堅牢。読み取りは Layer B 上問題なし（Orchestrator アクターの FileRead）。
- 116（ロジックバグ）対策は不要。`.chars().enumerate().rev()` は `expr.chars().collect::<Vec<_>>().into_iter().enumerate().rev()` で回避できるが、これはケース固有でありプロンプト一般化の対象外。

## 4. 検証手順（人間が修正適用後）

1. `cargo build --workspace`（稼働中プロセスを停止してから）。
2. orchestrator を起動し、既存モジュールが扱えない未知パターン（例: `sin(180)`, `log(0)` など Tier 2 新規追加を誘発する入力）を投入。
3. `metadata.db` の `modifications` で `tier=2` かつ `module_name` が既存4種以外、`build_result='success'`, `decision='adopted'` の行が**初めて**生成されることを確認。
4. `chain.toml` に新モジュールが `insert_after` 位置で追記され、再起動後パイプラインを通ることを確認。
5. 新モジュールに対する把握テスト（`comprehension_tests`）が `match` を返すこと。
