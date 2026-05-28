// Week 2/3 で段階的に接続される。未使用の dead_code を許容する。
#![allow(dead_code)]

// =============================================================================
// executor.rs — システム操作の抽象化レイヤー
//
// CmpLoop が直接依存する副作用 (cargo 実行・ファイル I/O・hot swap) を
// Executor trait で包む。本番では SystemExecutor、テストでは FakeExecutor を
// 差し込むことで、実プロセス・実 cargo を起動せずに Tier 1/2 ループを検証できる。
// =============================================================================

use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::process::Child;

use crate::hot_swap::HotSwapper;

/// cargo build の結果
pub struct BuildResult {
    pub success: bool,
    pub stderr: Option<String>,
}

/// システム操作の抽象インターフェース
#[async_trait]
pub trait Executor: Send + Sync {
    fn read_file(&self, path: &str) -> Result<String>;
    fn write_file(&self, path: &str, content: &str) -> Result<()>;
    fn copy_file(&self, src: &str, dst: &str) -> Result<()>;
    fn create_dir_all(&self, path: &str) -> Result<()>;
    fn remove_dir_all(&self, path: &str) -> Result<()>;
    async fn kill_child(&self, child: &mut Child) -> Result<()>;
    async fn spawn_process(&self, binary: &str, socket: &str) -> Result<Child>;
    async fn cargo_build(&self, pkg: &str) -> Result<BuildResult>;
    async fn cargo_build_repair(&self, pkg: &str) -> Result<BuildResult>;
    async fn cargo_test(&self, pkg: &str) -> Result<bool>;
    async fn cargo_test_repair(&self, pkg: &str) -> Result<bool>;
    async fn hot_swap(&self, swapper: &HotSwapper, old_child: Child) -> Result<Child>;
}

/// 本番実装: 実際の OS 操作を行う
pub struct SystemExecutor;

#[async_trait]
impl Executor for SystemExecutor {
    fn read_file(&self, path: &str) -> Result<String> {
        std::fs::read_to_string(path).with_context(|| format!("Cannot read {}", path))
    }

    fn write_file(&self, path: &str, content: &str) -> Result<()> {
        std::fs::write(path, content).map_err(|e| anyhow::anyhow!("Cannot write {}: {}", path, e))
    }

    fn copy_file(&self, src: &str, dst: &str) -> Result<()> {
        std::fs::copy(src, dst)
            .map(|_| ())
            .with_context(|| format!("Cannot copy {} → {}", src, dst))
    }

    fn create_dir_all(&self, path: &str) -> Result<()> {
        std::fs::create_dir_all(path).with_context(|| format!("Failed to create dir {}", path))
    }

    fn remove_dir_all(&self, path: &str) -> Result<()> {
        std::fs::remove_dir_all(path).with_context(|| format!("Failed to remove dir {}", path))
    }

    async fn kill_child(&self, child: &mut Child) -> Result<()> {
        child
            .kill()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to kill child process: {}", e))?;
        child
            .wait()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to wait for child process: {}", e))?;
        Ok(())
    }

    async fn spawn_process(&self, binary: &str, socket: &str) -> Result<Child> {
        tokio::process::Command::new(binary)
            .arg(socket)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to spawn process {}: {}", binary, e))
    }

    async fn cargo_build(&self, pkg: &str) -> Result<BuildResult> {
        let out = tokio::process::Command::new("cargo")
            .args(["build", "-p", pkg])
            .output()
            .await
            .context("Failed to run cargo build")?;
        Ok(BuildResult {
            success: out.status.success(),
            stderr: if out.status.success() {
                None
            } else {
                Some(String::from_utf8_lossy(&out.stderr).to_string())
            },
        })
    }

    async fn cargo_build_repair(&self, pkg: &str) -> Result<BuildResult> {
        let out = tokio::process::Command::new("cargo")
            .args(["build", "-p", pkg, "--target-dir", "target_repair"])
            .output()
            .await
            .context("Failed to run cargo build (repair)")?;
        Ok(BuildResult {
            success: out.status.success(),
            stderr: if out.status.success() {
                None
            } else {
                Some(String::from_utf8_lossy(&out.stderr).to_string())
            },
        })
    }

    async fn cargo_test(&self, pkg: &str) -> Result<bool> {
        Ok(tokio::process::Command::new("cargo")
            .args(["test", "-p", pkg])
            .output()
            .await
            .context("Failed to run cargo test")?
            .status
            .success())
    }

    async fn cargo_test_repair(&self, pkg: &str) -> Result<bool> {
        let out = tokio::process::Command::new("cargo")
            .args(["test", "-p", pkg, "--target-dir", "target_repair"])
            .output()
            .await
            .context("Failed to run cargo test (repair)")?;

        if !out.status.success() {
            let log_content = format!(
                "STDOUT:\n{}\nSTDERR:\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            let _ = std::fs::create_dir_all("target_repair");
            let _ = std::fs::write("target_repair/test_failure.log", log_content);
        } else {
            let _ = std::fs::remove_file("target_repair/test_failure.log");
        }

        Ok(out.status.success())
    }

    async fn hot_swap(&self, swapper: &HotSwapper, old_child: Child) -> Result<Child> {
        swapper.swap(old_child).await
    }
}
