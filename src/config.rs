use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PunchConfig {
    #[serde(default)]
    pub tunnels: Vec<PunchTunnelConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PunchTunnelConfig {
    pub domain: String,
    pub port: u16,
    pub https: Option<bool>,
    pub env: Option<String>,
}

impl PunchConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("读取配置文件失败: {}", path.display()))?;
        serde_yaml::from_str(&contents)
            .with_context(|| format!("解析配置文件失败: {}", path.display()))
    }
}
