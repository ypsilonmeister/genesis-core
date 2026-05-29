use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tracing::{info, warn};

pub struct ModuleProcess {
    #[allow(dead_code)]
    pub name: String,
    pub child: Child,
}

impl ModuleProcess {
    pub async fn spawn(name: &str, binary_path: &str, socket_path: &str) -> anyhow::Result<Self> {
        let mut child = Command::new(binary_path)
            .arg(socket_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let name_stdout = name.to_string();
        let name_stderr = name.to_string();

        if let Some(out) = stdout {
            tokio::spawn(async move {
                let mut reader = BufReader::new(out).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    info!(target: "module_stdout", module = %name_stdout, "{}", line);
                }
            });
        }

        if let Some(err) = stderr {
            tokio::spawn(async move {
                let mut reader = BufReader::new(err).lines();
                while let Ok(Some(line)) = reader.next_line().await {
                    warn!(target: "module_stderr", module = %name_stderr, "{}", line);
                }
            });
        }

        Ok(Self {
            name: name.to_string(),
            child,
        })
    }
}

