// =============================================================================
// chain.rs — モジュール呼び出しチェーンを chain.toml から読み込む
//
// Tier 2 で AI が chain.toml を書き換えてチェーンを変更する。
// 起動時の読み込みと、ホットリロード(SIGHUP 等)を担当する。
// =============================================================================

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct ChainConfig {
    pub modules: Vec<ModuleSpec>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModuleSpec {
    pub name: String,
    /// 起動するバイナリの相対パス (workspace target ディレクトリからの相対)
    pub binary: String,
    /// 通信に使う UDS パス
    pub socket: String,
}

impl ChainConfig {
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path)?;
        let config: ChainConfig = toml::from_str(&raw)?;
        Ok(config)
    }
}
