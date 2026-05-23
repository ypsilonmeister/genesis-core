// Week 3 で使われるスキャフォールディング。未使用の dead_code を許容する。
#![allow(dead_code)]

// =============================================================================
// attacker.rs — 攻撃 AI 呼び出し
//
// Lying Calculator §4 に従って Gemini に攻撃入力を生成させる。
// Phase A (文字レベル) → Phase D (構造レベル) を段階的に解放。
//
// charter/system.md §2 HI-4: 攻撃 AI は modules/, orchestrator/, charter/ の
// ソースコードに触れない。Layer B が AttackAi アクターを拒絶する。
//
// 現時点 (Week 2) はスタブ。Week 3 で本格実装:
//   - 攻撃プロンプトの構築
//   - gemini_backend.complete() で攻撃入力を取得
//   - JSON 配列としてパース
//   - チェーンに流して結果を attacks テーブルに記録
//   - diversity_score の計算と過適合検出 (§4.5)
// =============================================================================

use anyhow::Result;
use tracing::debug;

use crate::ai_backend::AiBackend;

pub struct Attacker {
    gemini: Box<dyn AiBackend>,
    pub phase: String,
}

impl Attacker {
    pub fn new(gemini: Box<dyn AiBackend>) -> Self {
        let phase = std::env::var("ATTACK_PHASE").unwrap_or_else(|_| "A".to_string());
        Self { gemini, phase }
    }

    // 攻撃入力を生成する。
    // success_samples: 直近の成功入力サンプル
    // recent_errors:   直近のエラーログ
    // 戻り値: 攻撃入力の文字列リスト
    // TODO(Week 3): 本格実装
    pub async fn generate_attacks(
        &self,
        success_samples: &[String],
        recent_errors: &[String],
    ) -> Result<Vec<String>> {
        debug!(phase = %self.phase, "Attacker::generate_attacks (stub)");

        // Week 3 実装ヒント:
        // let prompt = build_attack_prompt(&self.phase, success_samples, recent_errors);
        // let response = self.gemini.complete(&prompt).await?;
        // serde_json::from_str::<Vec<String>>(&response).context(...)

        let _ = (success_samples, recent_errors, &self.gemini);
        Ok(vec![])
    }
}
