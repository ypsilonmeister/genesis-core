// =============================================================================
// hot_swap.rs — モジュールバイナリの無停止差し替え
//
// CMP §8.2 Phase 1 の手順:
//   1. archive/ に旧バイナリを退避
//   2. 旧プロセスを kill
//   3. 新プロセスを同じ socket_path で起動
//   4. ヘルスチェック通過後、新 Child を返す
//
// 失敗時のロールバック規律は charter/system.md §7 を参照。
// =============================================================================

use anyhow::{bail, Context, Result};
use compat::UnixStream;
use std::path::Path;
use tokio::process::Child;
use tracing::{info, warn};

pub struct HotSwapper {
    pub module_name: String,
    pub binary_path: String,
    pub socket_path: String,
}

impl HotSwapper {
    pub fn new(module_name: &str, binary_path: &str, socket_path: &str) -> Self {
        Self {
            module_name: module_name.to_string(),
            binary_path: binary_path.to_string(),
            socket_path: socket_path.to_string(),
        }
    }

    /// 旧プロセスを archive に退避し、新プロセスに差し替える。
    /// 成功すると新プロセスの Child を返す。
    pub async fn swap(&self, mut old_child: Child) -> Result<Child> {
        info!(module = %self.module_name, "hot_swap: starting");

        // 1. 旧バイナリを archive/ に退避
        self.archive_old_binary()?;

        // 2. 旧プロセスを kill
        old_child
            .kill()
            .await
            .context("Failed to kill old process")?;
        old_child
            .wait()
            .await
            .context("Failed to wait for old process")?;
        info!(module = %self.module_name, "hot_swap: old process terminated");

        // 3. 既存の socket ファイルを削除 (残留すると bind できない)
        if Path::new(&self.socket_path).exists() {
            std::fs::remove_file(&self.socket_path).context("Failed to remove stale socket")?;
        }

        // 4. 新プロセスを起動
        let new_proc = crate::process::ModuleProcess::spawn(&self.module_name, &self.binary_path, &self.socket_path).await?;
        let new_child = new_proc.child;

        info!(module = %self.module_name, "hot_swap: new process spawned");

        // 5. ヘルスチェック (socket に繋がるまで最大 5 秒)
        self.health_check().await?;

        info!(module = %self.module_name, "hot_swap: completed successfully");
        Ok(new_child)
    }

    fn archive_old_binary(&self) -> Result<()> {
        let binary = Path::new(&self.binary_path);
        if !binary.exists() {
            return Ok(());
        }

        std::fs::create_dir_all("archive").context("Failed to create archive/")?;

        let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S").to_string();
        let archive_path = format!("archive/{}_{}", self.module_name, ts);

        std::fs::copy(&self.binary_path, &archive_path)
            .with_context(|| format!("Failed to archive binary to {}", archive_path))?;

        info!(module = %self.module_name, dest = %archive_path, "hot_swap: old binary archived");
        Ok(())
    }

    async fn health_check(&self) -> Result<()> {
        const MAX_RETRIES: u32 = 10;
        const RETRY_INTERVAL_MS: u64 = 500;

        for attempt in 1..=MAX_RETRIES {
            match UnixStream::connect(&self.socket_path).await {
                Ok(_) => {
                    info!(module = %self.module_name, attempt, "hot_swap: health check passed");
                    return Ok(());
                }
                Err(_) => {
                    warn!(
                        module = %self.module_name,
                        attempt,
                        max = MAX_RETRIES,
                        "hot_swap: waiting for socket"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(RETRY_INTERVAL_MS)).await;
                }
            }
        }

        bail!(
            "hot_swap health check failed: {} did not become ready within {}ms",
            self.module_name,
            MAX_RETRIES as u64 * RETRY_INTERVAL_MS
        );
    }
}



