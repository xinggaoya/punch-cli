use std::path::PathBuf;

use clap::{ArgAction, Parser, Subcommand};

use crate::types::LocalProtocol;

#[derive(Debug, Parser)]
#[command(
    name = "punch",
    about = "三秒打洞，本地服务即刻上线",
    version = env!("CARGO_PKG_VERSION"),
    disable_version_flag = true,
    propagate_version = true
)]
pub struct Cli {
    #[arg(value_name = "DOMAIN[:PORT]")]
    pub target: Option<String>,

    #[command(subcommand)]
    pub command: Option<Commands>,

    #[arg(long, short = 'V', global = true, action = ArgAction::SetTrue)]
    pub version: bool,

    #[arg(long, global = true, value_enum, conflicts_with_all = ["https", "tcp"])]
    pub protocol: Option<LocalProtocol>,

    #[arg(long, global = true, conflicts_with_all = ["protocol", "tcp"])]
    pub http: bool,

    #[arg(long, global = true, conflicts_with_all = ["protocol", "http", "tcp"])]
    pub https: bool,

    #[arg(long, global = true, conflicts_with_all = ["protocol", "http", "https"])]
    pub tcp: bool,

    #[arg(long, global = true, action = ArgAction::SetTrue)]
    pub insecure: bool,

    #[arg(long, global = true, action = ArgAction::SetTrue)]
    pub export: bool,

    #[arg(long, global = true, action = ArgAction::SetTrue)]
    pub detach: bool,

    #[arg(long, global = true, hide = true)]
    pub home: Option<PathBuf>,
}

impl Cli {
    pub fn selected_protocol(&self) -> Option<LocalProtocol> {
        self.protocol.or_else(|| {
            if self.http {
                Some(LocalProtocol::Http)
            } else if self.https {
                Some(LocalProtocol::Https)
            } else if self.tcp {
                Some(LocalProtocol::Tcp)
            } else {
                None
            }
        })
    }
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// 登录或切换 Cloudflare API Token
    Auth { token: Option<String> },
    /// 列出活跃隧道
    Ls,
    /// 停止指定隧道
    Stop { domain: String },
    /// 删除隧道并清理 DNS
    Rm { domain: String },
    /// 查看日志
    Logs {
        domain: Option<String>,
        #[arg(long, action = ArgAction::SetTrue)]
        follow: bool,
        #[arg(long, default_value_t = 50)]
        lines: usize,
    },
    /// 环境诊断
    Doctor,
    /// 从 punch.yml 批量启动
    Up {
        #[arg(long, default_value = "punch.yml")]
        file: PathBuf,
    },
    /// 分享模式（实验性，当前只登记元数据）
    Share {
        target: String,
        #[arg(long)]
        expires: String,
        #[arg(long)]
        password: String,
    },
    /// 导出 Prometheus 指标
    Metrics {
        #[arg(long, default_value_t = 9090)]
        port: u16,
    },
}
