// =============================================================================
// attacker.rs — 攻撃 AI 呼び出し
//
// Lying Calculator §4 に従って Gemini に攻撃入力を生成させる。
// Phase A (文字レベル) → Phase D (構造レベル) を段階的に解放。
//
// charter/system.md §2 HI-4: 攻撃 AI は modules/, orchestrator/, charter/ の
// ソースコードに触れない。Layer B が AttackAi アクターを拒絶する。
// =============================================================================

use anyhow::{Context, Result};
use tracing::{info, warn};

use crate::ai_backend::AiBackend;

pub struct Attacker {
    gemini: Box<dyn AiBackend>,
    pub phase: String,
    #[allow(dead_code)]
    pub model_name: String,
}

impl Attacker {
    pub fn new(gemini: Box<dyn AiBackend>) -> Self {
        let phase = std::env::var("ATTACK_PHASE").unwrap_or_else(|_| "A".to_string());
        let model_name = std::env::var("GEMINI_MODEL").unwrap_or_else(|_| "gemini-cli".to_string());
        Self {
            gemini,
            phase,
            model_name,
        }
    }

    /// 攻撃入力を生成する。
    ///
    /// - `success_samples`      : 直近の成功入力 (最大 5 件)
    /// - `recent_errors`        : 直近のエラー入力 (最大 10 件)
    /// - `recent_attack_inputs` : 直近 ~100 件の攻撃入力 (diversity 計算用)
    ///
    /// 戻り値: (攻撃入力リスト, diversity_score)
    pub async fn generate_attacks(
        &self,
        success_samples: &[String],
        recent_errors: &[String],
        recent_attack_inputs: &[String],
    ) -> Result<(Vec<String>, f64)> {
        let diversity = compute_diversity_score(recent_attack_inputs);
        info!(phase = %self.phase, diversity, "attacker: generating attacks");

        let prompt = self.build_prompt(success_samples, recent_errors, diversity);

        let response = self
            .gemini
            .complete(&prompt)
            .await
            .context("Gemini failed to generate attacks")?;

        // JSON 配列を抽出してパース
        let json_str = extract_json_array(&response);
        let inputs: Vec<String> = serde_json::from_str(&json_str)
            .with_context(|| format!("Failed to parse attack JSON: {}", json_str))?;

        if inputs.is_empty() {
            warn!("attacker: empty attack list returned");
        } else {
            info!(count = inputs.len(), "attacker: generated attacks");
        }

        Ok((inputs, diversity))
    }

    fn build_prompt(
        &self,
        success_samples: &[String],
        recent_errors: &[String],
        diversity: f64,
    ) -> String {
        let success_str = if success_samples.is_empty() {
            "なし".to_string()
        } else {
            success_samples
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        };

        let errors_str = if recent_errors.is_empty() {
            "なし".to_string()
        } else {
            recent_errors
                .iter()
                .take(10)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        };

        let diversity_hint = if diversity < 0.3 {
            "\n\n重要: 直近の攻撃パターンが単調です。これまでと全く異なる文字・記号・構造を試みてください。"
        } else {
            ""
        };

        let phase_examples = match self.phase.as_str() {
            "A" => "- 全角数字: \"３ + ５ * ２\"\n- 全角演算子: \"3 ＋ 5 × 2\"\n- 全角括弧: \"（3 + 5）* 2\"\n- ゼロ幅スペース混入\n- 余分な空白・改行: \"3  +\\n5 *  2\"",
            "B" => "- 通貨記号: \"$3 + €5\"\n- 単位混入: \"3kg + 5kg\"\n- パーセント: \"50% + 30%\"",
            "C" => "- 自然言語: \"three plus five times two\"\n- 日本語: \"三足す五かける二\"\n- 混合: \"3 plus 5 * 2\"",
            "D" => "- 未知関数: \"sin(30) + cos(60)\"\n- べき乗: \"2^10\"\n- 対数: \"log(100)\"",
            _ => "- 全角文字、不可視文字、余分な空白",
        };

        format!(
            "あなたは数式パーサーのファジングエージェントです。\
            以下の計算機システムを壊す入力を生成してください。\n\n\
            対応済みパターン: {success_str}\n\
            直近のエラーを引き起こした入力: {errors_str}\n\n\
            目標: これまでに成功していない新しい失敗パターンを発見する。\
            同じパターンの繰り返しは避けること。\n\n\
            Phase {phase} 攻撃例:\n{phase_examples}\
            {diversity_hint}\n\n\
            出力形式: JSON配列のみ。説明不要。1〜5個の入力を生成してください。\n\
            例: [\"３ + ５\", \"3　+　5\"]",
            success_str = success_str,
            errors_str = errors_str,
            phase = self.phase,
            phase_examples = phase_examples,
            diversity_hint = diversity_hint,
        )
    }
}

/// 直近の攻撃入力リストから diversity score を計算する。
///
/// score = ユニーク文字数 / 総文字数 (全入力を結合して計算)
/// score が低い = 攻撃パターンが単調
pub fn compute_diversity_score(inputs: &[String]) -> f64 {
    let recent: Vec<&String> = inputs.iter().rev().take(100).collect();
    if recent.is_empty() {
        return 1.0; // 初回は最大スコア
    }

    let all_chars: String = recent.iter().flat_map(|s| s.chars()).collect();
    let total = all_chars.chars().count();
    if total == 0 {
        return 0.0;
    }

    let unique: std::collections::HashSet<char> = all_chars.chars().collect();
    unique.len() as f64 / total as f64
}

/// Gemini のレスポンスから JSON 配列部分を抽出する。
fn extract_json_array(response: &str) -> String {
    let s = response.trim();
    // ```json\n...\n``` または ``` ... ``` を剥がす
    let inner = if let Some(rest) = s.strip_prefix("```json") {
        rest.trim_start_matches('\n').trim_end_matches("```").trim()
    } else if let Some(rest) = s.strip_prefix("```") {
        rest.trim_start_matches('\n').trim_end_matches("```").trim()
    } else {
        s
    };

    // '[' から ']' を探す
    if let (Some(start), Some(end)) = (inner.find('['), inner.rfind(']')) {
        inner[start..=end].to_string()
    } else {
        // フォールバック: そのまま返す
        inner.to_string()
    }
}

/// 攻撃インターバル (秒) をランダムに決定する。
/// ATTACK_INTERVAL_MIN_SECS / ATTACK_INTERVAL_MAX_SECS で設定可能。
pub fn rand_attack_delay_secs() -> u64 {
    let min = std::env::var("ATTACK_INTERVAL_MIN_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30u64);
    let max = std::env::var("ATTACK_INTERVAL_MAX_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(180u64);

    if min >= max {
        return min;
    }

    // SystemTime のナノ秒下位ビットで簡易ランダム
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;

    min + (nanos % (max - min))
}
