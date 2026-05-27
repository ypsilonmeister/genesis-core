# Tier 3 提案：修復AI へのプロンプト長超過対応

## 問題

Gemini CLI 呼び出し時に「The command line is too long」エラーが発生。
- Windows コマンドラインの制限：8191文字
- 現在の実装：`gemini -p "<prompt>"` でプロンプト全体をコマンドライン引数で渡す
- 修復プロンプトが数KB～数十KB に達し、制限を超過

ログ例：
```
2026-05-27T02:48:23.282906Z WARN ai_backend: primary AI failed
error: gemini cli exited with exit code: 1: The command line is too long.
```

## 根本原因

`orchestrator/src/ai_backend.rs:99` で CLI に渡す引数が長すぎる：
```rust
let output = create_command(&self.binary)
    .args(["-p", prompt, "-y"])  // ← prompt が 8191 文字超過時に失敗
```

## 3つの対応案

### 案A：ファイル経由（推奨）

修復プロンプトを一時ファイルに書き込み、ファイルパスを CLI に渡す。

**変更箇所：**
- `orchestrator/src/ai_backend.rs` の `GeminiCli::complete()` メソッド

**変更内容：**
```rust
impl AiBackend for GeminiCli {
    async fn complete(&self, prompt: &str) -> Result<String> {
        // 一時ファイルを作成
        let temp_file = std::env::temp_dir().join(format!(
            "gemini_prompt_{}.txt",
            uuid::Uuid::new_v4()
        ));
        tokio::fs::write(&temp_file, prompt).await?;
        
        // ファイル経由で呼び出し
        let output = create_command(&self.binary)
            .args(["-f", temp_file.to_str().unwrap(), "-y"])
            .output()
            .await?;
            
        // クリーンアップ
        let _ = tokio::fs::remove_file(&temp_file).await;
        
        // 既存のエラーハンドリング続行...
    }
}
```

**メリット：**
- 容量制限なし
- 最小限の変更

**デメリット：**
- gemini CLI が `-f` フラグをサポートしているか確認が必要
- 一時ファイル生成・削除のオーバーヘッド

---

### 案B：stdin 経由

修復プロンプトを CLI の stdin に渡す。

**変更内容：**
```rust
let mut child = create_command(&self.binary)
    .arg("-y")
    .stdin(std::process::Stdio::piped())
    .stdout(std::process::Stdio::piped())
    .spawn()?;
    
let mut stdin = child.stdin.take().unwrap();
stdin.write_all(prompt.as_bytes()).await?;
drop(stdin);

let output = child.wait_with_output().await?;
```

**メリット：**
- ファイル I/O 不要
- 効率的

**デメリット：**
- gemini CLI が stdin サポートしているか未確認
- エラーハンドリングが複雑化

---

### 案C：プロンプト圧縮（Tier 1 で対応）

修復AI に渡すプロンプトのサイズを削減する。

**変更箇所：** `orchestrator/src/cmp_loop.rs` の初期プロンプト生成

**現在の構造：**
```rust
let initial_prompt = format!(
    "Module Charter:\n{module_charter}\n\n\
    現在のコード:\n```rust\n{module_code}\n```\n\n\
    ...",
    module_charter,
    module_code,  // ← 数百～数千行、数KB
);
```

**圧縮案：**
- モジュール全体ではなく、エラー周辺のみを抽出（±50行程度）
- Module Charter は保持（必須）
- エラーコード・発生回数・モジュール名は保持

**メリット：**
- 容量削減効果大（50～80%削減見込み）
- 追加の I/O なし

**デメリット：**
- 修復AI が局所的な文脈のみで判断 → 不正確性リスク
- エラー周辺の特定ロジック追加必要

---

## 推奨対応：**案A + 案C**

1. **即座（Tier 1）**: Tier 1 で既に実装済みの parser エラーメッセージ短縮を検証
2. **短期（Tier 1→Tier 2）**: プロンプト圧縮ロジックを cmp_loop.rs に追加提案
3. **長期（Tier 3）**: ai_backend.rs のファイル経由化で完全対応

## 実装優先度

1. **✓ 完了**: parser のエラーメッセージ短縮（Tier 1）
2. **→ 次**: Case-by-case で case 分析 → 案A (ファイル経由) を精査
3. **提案待機**: Tier 3 承認後、ai_backend.rs を修正

---

## ユーザーへの確認事項

- [ ] gemini CLI が `-f` フラグをサポートしているか？
- [ ] stdin サポート確認済みか？
- [ ] 案A, B, C のいずれを優先するか？
