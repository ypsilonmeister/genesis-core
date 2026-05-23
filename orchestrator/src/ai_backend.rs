// Week 2/3 で段階的に使われるスキャフォールディング。未使用の dead_code を許容する。
#![allow(dead_code)]

// =============================================================================
// ai_backend.rs — AI バックエンド抽象化レイヤー
//
// 修復 AI (Claude) と攻撃 AI (Gemini) を共通の AiBackend trait で抽象化する。
// バックエンドは環境変数で切り替え:
//   CLAUDE_BACKEND=cli (default) | api
//   GEMINI_BACKEND=cli (default) | api
//
// CLI モード: claude/gemini コマンドをサブプロセスとして呼び出す。
//   - API キー不要
//   - claude -p "<prompt>" / gemini -p "<prompt>"
//
// API モード: reqwest で各社 HTTP API を叩く。
//   - ANTHROPIC_API_KEY / GEMINI_API_KEY が必要
// =============================================================================

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde::Deserialize;

#[async_trait]
pub trait AiBackend: Send + Sync {
    async fn complete(&self, prompt: &str) -> Result<String>;
}

// =============================================================================
// CLI 実装
// =============================================================================

pub struct ClaudeCli {
    pub binary: String,
}

impl Default for ClaudeCli {
    fn default() -> Self {
        Self { binary: "claude".to_string() }
    }
}

#[async_trait]
impl AiBackend for ClaudeCli {
    async fn complete(&self, prompt: &str) -> Result<String> {
        let output = tokio::process::Command::new(&self.binary)
            .args(["-p", prompt])
            .output()
            .await
            .with_context(|| format!("Failed to run '{}'", self.binary))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("claude cli exited with {}: {}", output.status, stderr.trim());
        }

        let text = String::from_utf8(output.stdout)
            .context("claude cli output was not valid UTF-8")?;
        Ok(text.trim().to_string())
    }
}

pub struct GeminiCli {
    pub binary: String,
}

impl Default for GeminiCli {
    fn default() -> Self {
        Self { binary: "gemini".to_string() }
    }
}

#[async_trait]
impl AiBackend for GeminiCli {
    async fn complete(&self, prompt: &str) -> Result<String> {
        let output = tokio::process::Command::new(&self.binary)
            .args(["-p", prompt])
            .output()
            .await
            .with_context(|| format!("Failed to run '{}'", self.binary))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("gemini cli exited with {}: {}", output.status, stderr.trim());
        }

        let text = String::from_utf8(output.stdout)
            .context("gemini cli output was not valid UTF-8")?;
        Ok(text.trim().to_string())
    }
}

// =============================================================================
// API 実装
// =============================================================================

pub struct ClaudeApi {
    pub api_key: String,
    pub model: String,
    client: reqwest::Client,
}

impl ClaudeApi {
    pub fn new(api_key: String, model: String) -> Self {
        Self { api_key, model, client: reqwest::Client::new() }
    }
}

#[derive(Deserialize)]
struct ClaudeResponse {
    content: Vec<ClaudeContent>,
}

#[derive(Deserialize)]
struct ClaudeContent {
    #[serde(rename = "type")]
    kind: String,
    text: String,
}

#[async_trait]
impl AiBackend for ClaudeApi {
    async fn complete(&self, prompt: &str) -> Result<String> {
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 4096,
            "messages": [{"role": "user", "content": prompt}]
        });

        let resp = self.client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .context("Anthropic API request failed")?;

        if !resp.status().is_success() {
            bail!("Anthropic API returned {}", resp.status());
        }

        let data: ClaudeResponse = resp.json().await.context("Failed to parse Anthropic response")?;
        let text = data.content.into_iter()
            .find(|c| c.kind == "text")
            .map(|c| c.text)
            .unwrap_or_default();
        Ok(text.trim().to_string())
    }
}

pub struct GeminiApi {
    pub api_key: String,
    pub model: String,
    client: reqwest::Client,
}

impl GeminiApi {
    pub fn new(api_key: String, model: String) -> Self {
        Self { api_key, model, client: reqwest::Client::new() }
    }
}

#[derive(Deserialize)]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    content: GeminiContent,
}

#[derive(Deserialize)]
struct GeminiContent {
    parts: Vec<GeminiPart>,
}

#[derive(Deserialize)]
struct GeminiPart {
    text: String,
}

#[async_trait]
impl AiBackend for GeminiApi {
    async fn complete(&self, prompt: &str) -> Result<String> {
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent?key={}",
            self.model, self.api_key
        );
        let body = serde_json::json!({
            "contents": [{"parts": [{"text": prompt}]}]
        });

        let resp = self.client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("Gemini API request failed")?;

        if !resp.status().is_success() {
            bail!("Gemini API returned {}", resp.status());
        }

        let data: GeminiResponse = resp.json().await.context("Failed to parse Gemini response")?;
        let text = data.candidates.into_iter()
            .next()
            .and_then(|c| c.content.parts.into_iter().next())
            .map(|p| p.text)
            .unwrap_or_default();
        Ok(text.trim().to_string())
    }
}

// =============================================================================
// ファクトリ
// =============================================================================

pub fn build_claude_backend() -> Result<Box<dyn AiBackend>> {
    let mode = std::env::var("CLAUDE_BACKEND").unwrap_or_else(|_| "cli".to_string());
    match mode.as_str() {
        "cli" => {
            let binary = std::env::var("CLAUDE_BINARY").unwrap_or_else(|_| "claude".to_string());
            Ok(Box::new(ClaudeCli { binary }))
        }
        "api" => {
            let key = std::env::var("ANTHROPIC_API_KEY")
                .context("ANTHROPIC_API_KEY is required when CLAUDE_BACKEND=api")?;
            let model = std::env::var("CLAUDE_MODEL")
                .unwrap_or_else(|_| "claude-sonnet-4-6".to_string());
            Ok(Box::new(ClaudeApi::new(key, model)))
        }
        other => bail!("Unknown CLAUDE_BACKEND value: '{}' (expected 'cli' or 'api')", other),
    }
}

pub fn build_gemini_backend() -> Result<Box<dyn AiBackend>> {
    let mode = std::env::var("GEMINI_BACKEND").unwrap_or_else(|_| "cli".to_string());
    match mode.as_str() {
        "cli" => {
            let binary = std::env::var("GEMINI_BINARY").unwrap_or_else(|_| "gemini".to_string());
            Ok(Box::new(GeminiCli { binary }))
        }
        "api" => {
            let key = std::env::var("GEMINI_API_KEY")
                .context("GEMINI_API_KEY is required when GEMINI_BACKEND=api")?;
            let model = std::env::var("GEMINI_MODEL")
                .unwrap_or_else(|_| "gemini-2.5-flash".to_string());
            Ok(Box::new(GeminiApi::new(key, model)))
        }
        other => bail!("Unknown GEMINI_BACKEND value: '{}' (expected 'cli' or 'api')", other),
    }
}
