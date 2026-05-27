// Week 2/3 で段階的に使われるスキャフォールディング。未使用の dead_code を許容する。
#![allow(dead_code)]

// =============================================================================
// ai_backend.rs — AI バックエンド抽象化レイヤー
//
// 修復 AI (REPAIR) と攻撃 AI (ATTACK) を共通の AiBackend trait で抽象化する。
// バックエンドは環境変数で切り替え:
//   REPAIR_BACKEND=claude (default) | gemini | agy | api
//   ATTACK_BACKEND=gemini (default) | claude | agy | api
//
// CLI モード: 各 CLI コマンドをサブプロセスとして呼び出す。
//   - API キー不要
//   - claude -p "<prompt>" / gemini -p "<prompt>" -y / agy -p "<prompt>" --dangerously-skip-permissions
//
// API モード: reqwest で各社 HTTP API を叩く。
//   - ANTHROPIC_API_KEY / GEMINI_API_KEY が必要
// =============================================================================

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use serde::Deserialize;
use tracing::warn;

fn create_command(binary: &str) -> tokio::process::Command {
    #[cfg(windows)]
    {
        let mut cmd = tokio::process::Command::new("cmd");
        cmd.arg("/C").arg(binary);
        cmd
    }
    #[cfg(not(windows))]
    {
        tokio::process::Command::new(binary)
    }
}

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
        Self {
            binary: "claude".to_string(),
        }
    }
}

#[async_trait]
impl AiBackend for ClaudeCli {
    async fn complete(&self, prompt: &str) -> Result<String> {
        let mut child = create_command(&self.binary)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to spawn '{}'", self.binary))?;

        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin
                .write_all(prompt.as_bytes())
                .await
                .context("Failed to write prompt to stdin")?;
        }

        let output = child
            .wait_with_output()
            .await
            .context("Failed to wait for command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "claude cli exited with {}: {}",
                output.status,
                stderr.trim()
            );
        }

        let text =
            String::from_utf8(output.stdout).context("claude cli output was not valid UTF-8")?;
        Ok(text.trim().to_string())
    }
}

pub struct GeminiCli {
    pub binary: String,
}

impl Default for GeminiCli {
    fn default() -> Self {
        Self {
            binary: "gemini".to_string(),
        }
    }
}

#[async_trait]
impl AiBackend for GeminiCli {
    async fn complete(&self, prompt: &str) -> Result<String> {
        let mut child = create_command(&self.binary)
            .arg("-y")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to spawn '{}'", self.binary))?;

        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin
                .write_all(prompt.as_bytes())
                .await
                .context("Failed to write prompt to stdin")?;
        }

        let output = child
            .wait_with_output()
            .await
            .context("Failed to wait for command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "gemini cli exited with {}: {}",
                output.status,
                stderr.trim()
            );
        }

        let text =
            String::from_utf8(output.stdout).context("gemini cli output was not valid UTF-8")?;
        Ok(text.trim().to_string())
    }
}

pub struct AgyCli {
    pub binary: String,
}

impl Default for AgyCli {
    fn default() -> Self {
        Self {
            binary: "agy".to_string(),
        }
    }
}

#[async_trait]
impl AiBackend for AgyCli {
    async fn complete(&self, prompt: &str) -> Result<String> {
        let mut child = create_command(&self.binary)
            .arg("--dangerously-skip-permissions")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .with_context(|| format!("Failed to spawn '{}'", self.binary))?;

        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            stdin
                .write_all(prompt.as_bytes())
                .await
                .context("Failed to write prompt to stdin")?;
        }

        let output = child
            .wait_with_output()
            .await
            .context("Failed to wait for command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("agy cli exited with {}: {}", output.status, stderr.trim());
        }

        let text =
            String::from_utf8(output.stdout).context("agy cli output was not valid UTF-8")?;
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
        Self {
            api_key,
            model,
            client: reqwest::Client::new(),
        }
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

        let resp = self
            .client
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

        let data: ClaudeResponse = resp
            .json()
            .await
            .context("Failed to parse Anthropic response")?;
        let text = data
            .content
            .into_iter()
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
        Self {
            api_key,
            model,
            client: reqwest::Client::new(),
        }
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
            "contents": [{"parts": [{"text": prompt}]}],
            "generationConfig": {
                "maxOutputTokens": 8192
            }
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("Gemini API request failed")?;

        if !resp.status().is_success() {
            bail!("Gemini API returned {}", resp.status());
        }

        let data: GeminiResponse = resp
            .json()
            .await
            .context("Failed to parse Gemini response")?;
        let text = data
            .candidates
            .into_iter()
            .next()
            .and_then(|c| c.content.parts.into_iter().next())
            .map(|p| p.text)
            .unwrap_or_default();
        Ok(text.trim().to_string())
    }
}

pub struct OllamaApi {
    pub host: String,
    pub model: String,
    client: reqwest::Client,
}

impl OllamaApi {
    pub fn new(host: String, model: String) -> Self {
        Self {
            host,
            model,
            client: reqwest::Client::new(),
        }
    }
}

#[derive(Deserialize)]
struct OllamaResponse {
    message: OllamaMessage,
}

#[derive(Deserialize)]
struct OllamaMessage {
    content: String,
}

#[async_trait]
impl AiBackend for OllamaApi {
    async fn complete(&self, prompt: &str) -> Result<String> {
        let url = format!("{}/api/chat", self.host.trim_end_matches('/'));
        let body = serde_json::json!({
            "model": self.model,
            "messages": [{"role": "user", "content": prompt}],
            "stream": false
        });

        let resp = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("Ollama API request failed")?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp.text().await.unwrap_or_default();
            bail!("Ollama API returned status {}: {}", status, err_text);
        }

        let data: OllamaResponse = resp
            .json()
            .await
            .context("Failed to parse Ollama response")?;

        Ok(data.message.content.trim().to_string())
    }
}

// =============================================================================
// フォールバック合成
// =============================================================================

/// primary が失敗したとき fallback に切り替えるラッパー。
pub struct FallbackBackend {
    primary: Box<dyn AiBackend>,
    fallback: Box<dyn AiBackend>,
    primary_name: String,
    fallback_name: String,
}

impl FallbackBackend {
    pub fn new(
        primary: Box<dyn AiBackend>,
        fallback: Box<dyn AiBackend>,
        primary_name: impl Into<String>,
        fallback_name: impl Into<String>,
    ) -> Self {
        Self {
            primary,
            fallback,
            primary_name: primary_name.into(),
            fallback_name: fallback_name.into(),
        }
    }
}

#[async_trait]
impl AiBackend for FallbackBackend {
    async fn complete(&self, prompt: &str) -> Result<String> {
        match self.primary.complete(prompt).await {
            Ok(r) => Ok(r),
            Err(e) => {
                warn!(
                    primary = %self.primary_name,
                    fallback = %self.fallback_name,
                    error = %e,
                    "primary AI failed, falling back"
                );
                self.fallback.complete(prompt).await
            }
        }
    }
}

// =============================================================================
// ファクトリ
// =============================================================================

fn build_backend(
    backend: &str,
    binary: Option<String>,
    api_provider: Option<String>,
    model: Option<String>,
    api_key: Option<String>,
) -> Result<Box<dyn AiBackend>> {
    match backend {
        "claude" | "cli" => {
            let bin = binary.unwrap_or_else(|| "claude".to_string());
            Ok(Box::new(ClaudeCli { binary: bin }))
        }
        "gemini" => {
            let bin = binary.unwrap_or_else(|| "gemini".to_string());
            Ok(Box::new(GeminiCli { binary: bin }))
        }
        "agy" => {
            let bin = binary.unwrap_or_else(|| "agy".to_string());
            Ok(Box::new(AgyCli { binary: bin }))
        }
        "ollama" => {
            let host = std::env::var("OLLAMA_HOST")
                .unwrap_or_else(|_| "http://localhost:11434".to_string());
            let m = model.unwrap_or_else(|| "qwen2.5-coder:7b".to_string());
            Ok(Box::new(OllamaApi::new(host, m)))
        }
        "api" => {
            let provider = api_provider
                .or_else(|| {
                    model.as_ref().and_then(|m| {
                        if m.contains("gemini") {
                            Some("google".to_string())
                        } else if m.contains("claude") {
                            Some("anthropic".to_string())
                        } else {
                            None
                        }
                    })
                })
                .context("API provider (google or anthropic) is required for api backend (either set REPAIR_API_PROVIDER/ATTACK_API_PROVIDER or use a model name containing 'gemini' or 'claude')")?;

            match provider.as_str() {
                "anthropic" => {
                    let key = api_key.context("API key is required for anthropic provider (REPAIR_API_KEY / ANTHROPIC_API_KEY or ATTACK_API_KEY)")?;
                    let m = model.unwrap_or_else(|| "claude-sonnet-4-6".to_string());
                    Ok(Box::new(ClaudeApi::new(key, m)))
                }
                "google" => {
                    let key = api_key.context("API key is required for google provider (REPAIR_API_KEY / GEMINI_API_KEY or ATTACK_API_KEY)")?;
                    let m = model.unwrap_or_else(|| "gemini-2.5-flash".to_string());
                    Ok(Box::new(GeminiApi::new(key, m)))
                }
                other => bail!("Unknown API provider: '{}'", other),
            }
        }
        other => bail!(
            "Unknown backend type: '{}' (expected 'claude', 'gemini', 'agy', 'ollama' or 'api')",
            other
        ),
    }
}

pub fn build_repair_backend() -> Result<Box<dyn AiBackend>> {
    let backend = std::env::var("REPAIR_BACKEND")
        .ok()
        .or_else(|| std::env::var("CLAUDE_BACKEND").ok())
        .unwrap_or_else(|| "claude".to_string());

    let binary = std::env::var("REPAIR_BINARY")
        .ok()
        .or_else(|| std::env::var("CLAUDE_BINARY").ok());

    let api_provider = std::env::var("REPAIR_API_PROVIDER").ok();
    let model = std::env::var("REPAIR_MODEL")
        .ok()
        .or_else(|| std::env::var("CLAUDE_MODEL").ok());

    let api_key = std::env::var("REPAIR_API_KEY")
        .ok()
        .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok());

    build_backend(&backend, binary, api_provider, model, api_key)
}

pub fn build_repair_fallback_backend() -> Result<Box<dyn AiBackend>> {
    let backend = std::env::var("REPAIR_FALLBACK_BACKEND")
        .ok()
        .unwrap_or_else(|| "gemini".to_string());

    let binary = std::env::var("REPAIR_BINARY")
        .ok()
        .or_else(|| std::env::var("GEMINI_BINARY").ok());

    let api_provider = std::env::var("REPAIR_FALLBACK_API_PROVIDER").ok();
    let model = std::env::var("REPAIR_FALLBACK_MODEL")
        .ok()
        .or_else(|| std::env::var("GEMINI_MODEL").ok());

    let api_key = std::env::var("REPAIR_FALLBACK_API_KEY")
        .ok()
        .or_else(|| std::env::var("GEMINI_API_KEY").ok());

    build_backend(&backend, binary, api_provider, model, api_key)
}

pub fn build_attack_backend() -> Result<Box<dyn AiBackend>> {
    let backend = std::env::var("ATTACK_BACKEND")
        .ok()
        .or_else(|| std::env::var("GEMINI_BACKEND").ok())
        .unwrap_or_else(|| "gemini".to_string());

    let binary = std::env::var("ATTACK_BINARY")
        .ok()
        .or_else(|| std::env::var("GEMINI_BINARY").ok());

    let api_provider = std::env::var("ATTACK_API_PROVIDER").ok();
    let model = std::env::var("ATTACK_MODEL")
        .ok()
        .or_else(|| std::env::var("GEMINI_MODEL").ok());

    let api_key = std::env::var("ATTACK_API_KEY")
        .ok()
        .or_else(|| std::env::var("GEMINI_API_KEY").ok());

    build_backend(&backend, binary, api_provider, model, api_key)
}
