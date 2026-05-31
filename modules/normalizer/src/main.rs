// =============================================================================
// # CMP Module Charter
//
// What:
//   Normalize the input string and pass it to the next module (whitespace removal only).
//
// Invariants:
//   - Do not destructively modify the input string (keep the original in logs)
//   - Return an error when an empty string is received
//
// Boundaries:
//   - Dependencies: none
//   - Dependents: tokenizer
//
// Extensible:
//   - Additional normalization rules (full-width to half-width, invisible char removal, etc.)
//
// Why:
//   Strip surface-level noise so that downstream modules can focus on pure parsing.
//
// When the AI modifies this in Tier 1, it must never break the Invariants and
// Boundaries above. Changes beyond the scope of What are handled as Tier 2.
// =============================================================================

// v1 scope:
//   Collapse consecutive whitespace into a single space and trim the edges.
//   Everything else passes through.

use anyhow::Result;
use compat::UnixListener;
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
    tracing::info!("normalizer booting (v1)");

    let socket_path = env::args()
        .nth(1)
        .unwrap_or_else(|| "/tmp/genesis-core/normalizer.sock".to_string());

    // Remove any pre-existing socket file
    let _ = std::fs::remove_file(&socket_path);
    // Create the directory
    if let Some(parent) = std::path::Path::new(&socket_path).parent() {
        let _ = std::fs::create_dir_all(parent);
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
                            code: "SYNTAX_ERROR".to_string(), // normalizer doesn't have specific error codes in spec yet
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

/// Collapse consecutive whitespace into a single space and trim the edges.
/// Does not destroy the original input (only borrows a reference).
/// Returns an error for an empty string.
pub fn normalize(input: &str) -> Result<String, String> {
    if input.trim().is_empty() {
        return Err("Empty input".to_string());
    }
    Ok(input.split_whitespace().collect::<Vec<_>>().join(" "))
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
    fn collapses_whitespace() {
        assert_eq!(normalize("3  +\n5 *  2").unwrap(), "3 + 5 * 2");
    }

    #[test]
    fn trims_edges() {
        assert_eq!(normalize("  3 + 5  ").unwrap(), "3 + 5");
    }

    #[test]
    fn empty_input_returns_error() {
        // Charter: return an error when an empty string is received
        assert!(normalize("").is_err());
        assert!(normalize("   ").is_err());
    }
}
