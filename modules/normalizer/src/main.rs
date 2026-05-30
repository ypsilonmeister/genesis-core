// =============================================================================
// # CMP Module Charter
//
// What:
//   Normalize the input string and pass it to subsequent modules (whitespace removal only).
//
// Invariants:
//   - Do not modify the input string destructively (the original input must be logged).
//   - Return an error if an empty string is received.
//
// Boundaries:
//   - Dependencies: None
//   - Dependents: tokenizer
//
// Extensible:
//   - Addition of normalization rules (e.g., full-width to half-width conversion, invisible character removal, etc.)
//
// Why:
//   Remove surface noise so that subsequent modules can focus on pure analysis.
//
// When modifying in Tier 1, the AI must never violate the above Invariants
// and Boundaries. Changes beyond the scope of "What" are treated as Tier 2.
// =============================================================================

// v2 実装範囲:
//   連続空白を単一空白に圧縮、前後空白をトリム。
//   全角半角変換、特殊記号(×, ÷等)の正規化、単語オペレータ(plus等)の置換、単位・通貨記号の除去をサポート。
//   数値のテキスト表現 (one, two等) や追加の数学関数 (asin, acos等) のサポートを拡充。

use compat::UnixListener;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::env;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleRequest {
    pub request_id: String,
    pub input: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleResponse {
    pub request_id: String,
    pub output: Option<String>,
    pub error: Option<ModuleError>,
    pub processing_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleError {
    pub code: String,
    pub message: String,
    pub input_position: Option<usize>,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    tracing::info!("normalizer booting (v2.4)");

    let socket_path = env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/genesis-core/normalizer.sock".to_string());

    // 以前のソケットファイルを削除
    let _ = std::fs::remove_file(&socket_path);
    // ディレクトリ作成
    if let Some(parent) = std::path::Path::new(&socket_path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let listener = UnixListener::bind(&socket_path)?;
    tracing::info!("Listening on {}", socket_path);

    loop {
        let (stream, _) = listener.accept().await?;
        tokio::spawn(async move {
            let (reader, mut writer) = tokio::io::split(stream);
            let mut reader = tokio::io::BufReader::new(reader);
            let mut line = String::new();

            if let Ok(n) = reader.read_line(&mut line).await {
                if n == 0 {
                    return;
                }
                let start = std::time::Instant::now();
                let request: ModuleRequest = match serde_json::from_str(&line) {
                    Ok(req) => req,
                    Err(e) => {
                        tracing::error!("Failed to parse request: {}", e);
                        return;
                    }
                };

                let (output, error) = match normalize(&request.input) {
                    Ok(out) => (Some(out), None),
                    Err(e) => (
                        None,
                        Some(ModuleError {
                            code: "UNKNOWN_PATTERN".to_string(),
                            message: e,
                            input_position: None,
                        }),
                    ),
                };

                let response = ModuleResponse {
                    request_id: request.request_id,
                    output,
                    error,
                    processing_ms: start.elapsed().as_millis() as u64,
                };

                if let Ok(payload) = serde_json::to_vec(&response) {
                    let mut payload = payload;
                    payload.push(b'\n');
                    let _ = writer.write_all(&payload).await;
                }
            }
        });
    }
}

/// 入力文字列を正規化する。
/// 1. 全角文字を半角に変換し、数学記号(×, ÷等)を標準化。
/// 2. 特殊演算子(**)の統一とテキスト形式の演算子(plus等)や数値(five等)の置換。
/// 3. 特殊記号(カッコ含む)の周りにスペースを挿入。
/// 4. 単位や通貨記号を除去し、数式に関係のあるトークンのみを残す。
pub fn normalize(input: &str) -> Result<String, String> {
    if input.trim().is_empty() {
        return Err("Empty input".to_string());
    }

    // 1. 全角半角変換と数学記号の正規化
    let s: String = input.chars().map(|c| {
        match c {
            '０'..='９' => char::from_u32(c as u32 - 0xFEE0).unwrap(),
            'Ａ'..='Ｚ' => char::from_u32(c as u32 - 0xFEE0).unwrap(),
            'ａ'..='ｚ' => char::from_u32(c as u32 - 0xFEE0).unwrap(),
            '＋' => '+',
            '－' | '−' | 'ー' | '―' | '‐' => '-',
            '×' | '✕' | '✖' | '＊' => '*',
            '÷' | '／' | '＼' => '/',
            '（' | '［' | '｛' => '(',
            '）' | '］' | '｝' => ')',
            '＾' => '^',
            '％' => '%',
            '　' => ' ',
            '，' => ',',
            '．' => '.',
            '：' => ':',
            '￥' | '＄' | '€' | '￡' | '￠' | '¥' | '$' => ' ',
            _ => c,
        }
    }).collect();

    // 2. 基本的なクリーニングと特殊演算子の統一
    let mut s = s.to_lowercase();
    while s.contains("**") {
        s = s.replace("**", "^");
    }
    s = s.replace("divided by", "/");
    s = s.replace("multiplied by", "*");

    // 3. 特殊記号の周りにスペースを挿入 (カッコを確実に分離し、後続のフィルタを通りやすくする)
    let mut spaced = String::new();
    for c in s.chars() {
        if "+-*/^()%,".contains(c) {
            spaced.push(' ');
            spaced.push(c);
            spaced.push(' ');
        } else {
            spaced.push(c);
        }
    }
    s = spaced;

    // 4. 単語レベルの処理 (置換、単位除去、ノイズフィルタ)
    let words: Vec<&str> = s.split_whitespace().collect();
    let mut final_tokens = Vec::new();
    let units = [
        "kg", "g", "mg", "m", "cm", "mm", "km", "s", "min", "h",
        "eur", "usd", "jpy", "gbp", "cny", "krw", "percent", "pcs",
        "yen", "dollar", "euro", "pound", "bucks"
    ];

    for word in words {
        let mut current = match word {
            "plus" | "add" | "and" => "+".to_string(),
            "minus" | "subtract" | "less" => "-".to_string(),
            "times" | "multiply" | "multiplied" => "*".to_string(),
            "over" | "divide" | "divided" => "/".to_string(),
            "modulo" | "mod" => "%".to_string(),
            "zero" => "0".to_string(),
            "one" => "1".to_string(),
            "two" => "2".to_string(),
            "three" => "3".to_string(),
            "four" => "4".to_string(),
            "five" => "5".to_string(),
            "six" => "6".to_string(),
            "seven" => "7".to_string(),
            "eight" => "8".to_string(),
            "nine" => "9".to_string(),
            "ten" => "10".to_string(),
            "hundred" => "100".to_string(),
            "thousand" => "1000".to_string(),
            _ => word.to_string(),
        };

        // 単位の除去 (例: 5.5kg -> 5.5)
        for unit in &units {
            if current.ends_with(unit) && current.len() > unit.len() {
                let prefix = &current[..current.len() - unit.len()];
                if prefix.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ',') {
                    current = prefix.to_string();
                    break;
                }
            }
        }

        // 許可された単語の検証 (関数、変数、定数)
        if !current.is_empty() && current.chars().all(|c| c.is_ascii_alphabetic()) {
            if units.contains(&current.as_str()) {
                continue;
            }
            let is_func = matches!(current.as_str(), 
                "log" | "ln" | "sin" | "cos" | "tan" | "sqrt" | "floor" | "ceil" | "abs" | "round" | 
                "asin" | "acos" | "atan" | "log10" | "exp" | "pow" | "cbrt" | "pi" | "e"
            );
            let is_var = current.len() == 1;
            if !is_func && !is_var {
                continue;
            }
        }

        // 最終的な文字フィルタ (カッコ ( ) を明示的に許可)
        let filtered: String = current.chars().filter(|c| {
            c.is_ascii_digit() || "+-*/^().,%()".contains(*c) || c.is_ascii_alphabetic()
        }).collect();

        if !filtered.is_empty() {
            final_tokens.push(filtered);
        }
    }

    let result = final_tokens.join(" ");
    if result.is_empty() {
        return Err("Normalization produced no valid tokens".to_string());
    }
    
    Ok(result)
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,normalizer=debug"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handled_patterns() {
        // （1 + 2）*（3 + 4）-1
        assert_eq!(normalize("（1 + 2）*（3 + 4）-1").unwrap(), "( 1 + 2 ) * ( 3 + 4 ) - 1");
        // 2**5 + 10
        assert_eq!(normalize("2**5 + 10").unwrap(), "2 ^ 5 + 10");
        // 7 + 5m
        assert_eq!(normalize("7 + 5m").unwrap(), "7 + 5");
        // (100/3) + 1.5
        assert_eq!(normalize("(100/3) + 1.5").unwrap(), "( 100 / 3 ) + 1.5");
        // 3^3 * log(10)
        assert_eq!(normalize("3^3 * log(10)").unwrap(), "3 ^ 3 * log ( 10 )");
    }

    #[test]
    fn basic_suite() {
        assert_eq!(normalize("３ + （５ × ２）").unwrap(), "3 + ( 5 * 2 )");
        assert_eq!(normalize("100€ / 5 kg").unwrap(), "100 / 5");
        assert_eq!(normalize("asin(1) + acos(0)").unwrap(), "asin ( 1 ) + acos ( 0 )");
    }
}