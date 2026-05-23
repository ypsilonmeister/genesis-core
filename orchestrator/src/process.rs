use std::process::Stdio;
use tokio::process::{Child, Command};

pub struct ModuleProcess {
    #[allow(dead_code)]
    pub name: String,
    pub child: Child,
}

impl ModuleProcess {
    pub async fn spawn(name: &str, binary_path: &str, socket_path: &str) -> anyhow::Result<Self> {
        let child = Command::new(binary_path)
            .arg(socket_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        Ok(Self {
            name: name.to_string(),
            child,
        })
    }
}
