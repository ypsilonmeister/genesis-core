// Week 2/3 で段階的に使われるスキャフォールディング。未使用の dead_code を許容する。
#![allow(dead_code)]

// =============================================================================
// cmp_loop.rs — CMP Tier 1 / Tier 2 自律改変ループ
//
// Lying Calculator §5 に従う:
//
//   Tier 1 トリガ: 同一エラーコードが N 回以上発生(デフォルト 3)
//     → claude_backend.complete(repair_prompt) で修復案を取得
//     → cargo build + cargo test で検証
//     → 通過したら hot_swap
//     → 失敗もメタデータに転写(無編集)
//
//   Tier 2 トリガ: UNKNOWN_PATTERN が N 回以上(デフォルト 5)
//     → 「既存拡張で対応可能か」を AI に問う
//     → 新モジュール必要なら対照群評価を実施
//     → 優位な案のみ採用
//
// 過適合への防御は CMP §7 を参照。
// =============================================================================

use anyhow::Result;
use tracing::info;

use crate::ai_backend::AiBackend;

pub struct CmpLoop {
    claude: Box<dyn AiBackend>,
    tier1_trigger: u32,
}

impl CmpLoop {
    pub fn new(claude: Box<dyn AiBackend>) -> Self {
        let tier1_trigger = std::env::var("TIER1_TRIGGER_COUNT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(3);
        Self { claude, tier1_trigger }
    }

    // Tier 1: 同一エラーコードが閾値回数以上発生したら Claude に修復案を要求する。
    // 戻り値: 修復案の文字列 (閾値未達の場合は None)
    // TODO(Week 2): cargo build + hot_swap + metadata 記録を追加
    pub async fn maybe_repair(
        &self,
        error_code: &str,
        error_count: u32,
        module_name: &str,
        module_code: &str,
        module_charter: &str,
    ) -> Result<Option<String>> {
        if error_count < self.tier1_trigger {
            return Ok(None);
        }

        let prompt = format!(
            "以下のRustモジュールがエラーを繰り返しています。\n\n\
            Module Charter:\n{}\n\n\
            エラーコード: {}\n\
            発生回数: {}\n\
            モジュール名: {}\n\n\
            現在のコード:\n{}\n\n\
            修復案を生成してください。\n\
            制約:\n\
            - Module CharterのInvariantsを破らないこと\n\
            - Module CharterのBoundariesを変更しないこと\n\
            - 修正範囲は最小限にすること\n\n\
            出力: 修正後のRustコード全体",
            module_charter, error_code, error_count, module_name, module_code
        );

        info!(
            module = %module_name,
            error_code = %error_code,
            count = error_count,
            "Tier 1: requesting repair from claude"
        );

        let response = self.claude.complete(&prompt).await?;
        info!(
            module = %module_name,
            chars = response.len(),
            "Tier 1: received repair proposal"
        );

        Ok(Some(response))
    }

    // TODO(Week 3): Tier 2 ループ実装
}
