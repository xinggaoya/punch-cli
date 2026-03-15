use std::fmt::{Display, Formatter};
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum LocalProtocol {
    Http,
    Https,
    Tcp,
}

impl LocalProtocol {
    pub fn service_url(self, port: u16) -> String {
        match self {
            Self::Http => format!("http://127.0.0.1:{port}"),
            Self::Https => format!("https://127.0.0.1:{port}"),
            Self::Tcp => format!("tcp://127.0.0.1:{port}"),
        }
    }

    pub fn scheme(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
            Self::Tcp => "tcp",
        }
    }
}

impl Display for LocalProtocol {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.scheme())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelTarget {
    pub domain: String,
    pub port: u16,
}

impl TunnelTarget {
    pub fn parse(value: &str) -> Result<Self> {
        let value = value.trim();
        if value.is_empty() {
            bail!("请输入域名，例如 `punch demo.com:8080`");
        }

        let (domain, port) = match value.rsplit_once(':') {
            Some((domain, port)) if !port.contains('.') => {
                let port = port
                    .parse::<u16>()
                    .with_context(|| format!("无效端口: {port}"))?;
                (domain.to_string(), port)
            }
            _ => (value.to_string(), 8080),
        };

        if domain.is_empty() || !domain.contains('.') {
            bail!("无效域名: {domain}");
        }

        Ok(Self { domain, port })
    }
}

impl FromStr for TunnelTarget {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelStatus {
    Running,
    Stopped,
    Failed,
    Unknown,
}

impl Display for TunnelStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        };
        f.write_str(value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareSettings {
    pub expires_at: DateTime<Utc>,
    pub password_hint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelRecord {
    pub domain: String,
    pub local_port: u16,
    pub local_protocol: LocalProtocol,
    pub zone_id: String,
    pub zone_name: String,
    pub account_id: String,
    pub tunnel_id: String,
    pub tunnel_name: String,
    pub tunnel_token: String,
    pub dns_record_id: Option<String>,
    pub dns_target: String,
    pub pid: Option<u32>,
    pub detached: bool,
    pub status: TunnelStatus,
    pub log_file: String,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub share: Option<ShareSettings>,
}

impl TunnelRecord {
    pub fn public_url(&self) -> String {
        format!("https://{}", self.domain)
    }

    pub fn local_target(&self) -> String {
        format!("{}://127.0.0.1:{}", self.local_protocol, self.local_port)
    }
}
