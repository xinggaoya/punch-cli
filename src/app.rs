use std::cmp::Reverse;
use std::env;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use owo_colors::OwoColorize;
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::cli::{Cli, Commands};
use crate::cloudflare::CloudflareClient;
use crate::cloudflared::{Cloudflared, follow_log_file, tail_log_file};
use crate::config::PunchConfig;
use crate::metrics::serve_metrics;
use crate::state::StateStore;
use crate::storage::{CredentialBackend, CredentialStore, PunchDirs};
use crate::types::{LocalProtocol, ShareSettings, TunnelRecord, TunnelStatus, TunnelTarget};

#[derive(Clone)]
struct App {
    dirs: PunchDirs,
    credentials: CredentialStore,
    state: StateStore,
}

#[derive(Debug, Clone)]
struct LaunchOptions {
    protocol: Option<LocalProtocol>,
    insecure: bool,
    detach: bool,
    export: bool,
    share: Option<ShareSettings>,
}

pub async fn run(cli: Cli) -> Result<()> {
    let app = App::new(cli.home.clone())?;
    let selected_protocol = cli.selected_protocol();
    let insecure = cli.insecure;

    if cli.version {
        return app.print_version().await;
    }

    match cli.command {
        Some(Commands::Auth { token }) => app.auth(token).await,
        Some(Commands::Ls) => app.list_tunnels().await,
        Some(Commands::Stop { domain }) => app.stop_tunnel(&domain).await,
        Some(Commands::Rm { domain }) => app.remove_tunnel(&domain).await,
        Some(Commands::Logs {
            domain,
            follow,
            lines,
        }) => app.show_logs(domain.as_deref(), follow, lines).await,
        Some(Commands::Doctor) => app.doctor().await,
        Some(Commands::Up { file }) => app.up(file, selected_protocol, insecure).await,
        Some(Commands::Share {
            target,
            expires,
            password,
        }) => {
            app.share(target, &expires, &password, selected_protocol, insecure)
                .await
        }
        Some(Commands::Metrics { port }) => serve_metrics(app.state.clone(), port).await,
        None => {
            let target = cli
                .target
                .as_deref()
                .ok_or_else(|| anyhow!("请输入目标，例如 `punch demo.com:8080`"))?;
            let options = LaunchOptions {
                protocol: selected_protocol,
                insecure: cli.insecure,
                detach: cli.detach || cli.export,
                export: cli.export,
                share: None,
            };
            app.launch(target, options).await
        }
    }
}

impl App {
    fn new(home_override: Option<PathBuf>) -> Result<Self> {
        let dirs = PunchDirs::discover(home_override)?;
        dirs.ensure()?;
        let credentials = CredentialStore::new(dirs.clone());
        let state = StateStore::new(dirs.clone());
        Ok(Self {
            dirs,
            credentials,
            state,
        })
    }

    async fn print_version(&self) -> Result<()> {
        println!("punch {}", env!("CARGO_PKG_VERSION"));
        match Cloudflared::detect().and_then(|client| client.version()) {
            Ok(version) => println!("cloudflared {version}"),
            Err(_) => println!("cloudflared 未安装"),
        }
        Ok(())
    }

    async fn auth(&self, token: Option<String>) -> Result<()> {
        let token = match token {
            Some(token) => token,
            None => self.prompt_token()?,
        };

        let cf = CloudflareClient::new(token.clone())?;
        let verify = cf.verify_token().await.context("凭证校验失败")?;
        if verify.status != "active" {
            bail!("✗ 凭证不可用，状态: {}", verify.status);
        }

        let backend = self.credentials.save(&token)?;
        let email = cf
            .current_user_email()
            .await
            .unwrap_or_else(|| "Cloudflare 用户".to_string());

        println!("{} 验证通过: {}", "✓".green(), email);
        match backend {
            CredentialBackend::KeyringWithFallback => {
                println!(
                    "{} 凭证已写入系统密钥环，并同步保存到本地回退文件: {}",
                    "✓".green(),
                    self.dirs.fallback_token_file().display()
                );
            }
            CredentialBackend::FallbackFile => {
                println!(
                    "{} 系统密钥环不可用，已保存到本地凭证文件: {}",
                    "⚠".yellow(),
                    self.dirs.fallback_token_file().display()
                );
            }
        }
        Ok(())
    }

    async fn launch(&self, target: &str, options: LaunchOptions) -> Result<()> {
        let target = TunnelTarget::parse(target)?;
        let cloudflared = Cloudflared::detect()?;
        let token = self.load_or_prompt_token().await?;
        let cf = CloudflareClient::new(token)?;

        let protocol = match options.protocol {
            Some(protocol) => {
                self.ensure_port_open(target.port).await?;
                protocol
            }
            None => self.detect_protocol(target.port, options.insecure).await?,
        };

        println!("{} 继续创建隧道...", "→".cyan());
        let zone = cf.resolve_zone(&target.domain).await?;
        println!("{} Zone: {}", "✓".green(), zone.name);

        let tunnel_name = tunnel_name_for(&target.domain);
        let tunnel = cf.ensure_tunnel(&zone.account.id, &tunnel_name).await?;
        println!(
            "{} 隧道: {} (uuid: {})",
            "✓".green(),
            tunnel.name,
            &tunnel.id
        );

        let service = protocol.service_url(target.port);
        cf.configure_tunnel(
            &zone.account.id,
            &tunnel.id,
            &target.domain,
            &service,
            options.insecure,
        )
        .await?;

        let dns_target = format!("{}.cfargotunnel.com", tunnel.id);
        let dns_record = cf
            .ensure_dns_record(&zone.id, &target.domain, &dns_target)
            .await?;
        println!(
            "{} DNS: {} → CNAME → {}",
            "✓".green(),
            target.domain,
            dns_target
        );

        let log_path = self.dirs.log_file_for(&target.domain);
        let mut record = TunnelRecord {
            domain: target.domain.clone(),
            local_port: target.port,
            local_protocol: protocol,
            zone_id: zone.id,
            zone_name: zone.name,
            account_id: zone.account.id,
            tunnel_id: tunnel.id.clone(),
            tunnel_name: tunnel.name,
            tunnel_token: tunnel.token.clone(),
            dns_record_id: Some(dns_record.id),
            dns_target,
            pid: None,
            detached: options.detach,
            status: TunnelStatus::Unknown,
            log_file: log_path.display().to_string(),
            created_at: Utc::now(),
            started_at: None,
            last_seen_at: None,
            share: options.share.clone(),
        };

        if options.detach {
            let pid = cloudflared.spawn_detached(&tunnel.token, &log_path)?;
            record.pid = Some(pid);
            record.status = TunnelStatus::Running;
            record.started_at = Some(Utc::now());
            record.last_seen_at = Some(Utc::now());
            self.state.upsert(record.clone())?;

            if options.export {
                println!("export PUNCH_URL={}", shell_quote(&record.public_url()));
                println!("export PUNCH_DOMAIN={}", shell_quote(&record.domain));
                println!(
                    "export PUNCH_PORT={}",
                    shell_quote(&record.local_port.to_string())
                );
                return Ok(());
            }

            self.print_launch_success(&record);
            println!("{} 后台运行中，PID: {}", "✓".green(), pid);
            println!("{} 日志文件: {}", "✓".green(), log_path.display());
            if record.share.is_some() {
                println!(
                    "{} 分享元数据已记录，但密码保护仍需后续接入 Cloudflare Access API",
                    "⚠".yellow()
                );
            }
            return Ok(());
        }

        let mut child = cloudflared.spawn_foreground(&tunnel.token)?;
        record.pid = child.id();
        record.status = TunnelStatus::Running;
        record.started_at = Some(Utc::now());
        record.last_seen_at = Some(Utc::now());
        self.state.upsert(record.clone())?;
        self.print_launch_success(&record);
        println!("{}", "按 Ctrl+C 停止".dimmed());

        let stdout = child.stdout.take().context("无法读取 cloudflared stdout")?;
        let stderr = child.stderr.take().context("无法读取 cloudflared stderr")?;
        let log_task = tokio::spawn(crate::cloudflared::pump_streams(
            stdout,
            stderr,
            PathBuf::from(&record.log_file),
            true,
        ));

        let wait_result = tokio::select! {
            status = child.wait() => {
                let status = status.context("等待 cloudflared 退出失败")?;
                if !status.success() {
                    Err(anyhow!("cloudflared 异常退出: {status}"))
                } else {
                    Ok(())
                }
            }
            _ = tokio::signal::ctrl_c() => {
                child.start_kill().context("停止 cloudflared 失败")?;
                let _ = child.wait().await;
                Ok(())
            }
        };

        let log_result = log_task.await.context("等待日志任务失败")?;
        let _ = log_result;

        // 前台运行只在活跃期间登记，退出后直接清理本地状态，避免 ls 混入临时会话。
        let _ = self.state.remove(&record.domain)?;

        wait_result
    }

    async fn list_tunnels(&self) -> Result<()> {
        let mut tunnels = self.state.refresh_statuses()?;
        tunnels.sort_by_key(|record| Reverse(record.started_at));

        if tunnels.is_empty() {
            println!("没有已登记的隧道。");
            return Ok(());
        }

        println!("ACTIVE TUNNELS");
        println!("──────────────");
        for tunnel in tunnels {
            let indicator = match tunnel.status {
                TunnelStatus::Running => "●".green().to_string(),
                TunnelStatus::Stopped => "○".dimmed().to_string(),
                TunnelStatus::Failed => "!".red().to_string(),
                TunnelStatus::Unknown => "?".yellow().to_string(),
            };
            let runtime = tunnel
                .started_at
                .map(format_runtime)
                .unwrap_or_else(|| "-".to_string());
            println!(
                "{} {:<24} {:<36} {:<8} pid={}",
                indicator,
                tunnel.domain,
                format!("{} → :{}", tunnel.public_url(), tunnel.local_port),
                runtime,
                tunnel
                    .pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "-".to_string())
            );
        }
        Ok(())
    }

    async fn stop_tunnel(&self, domain: &str) -> Result<()> {
        match self.state.stop_process(domain)? {
            Some(record) => {
                if !record.detached {
                    let _ = self.state.remove(domain)?;
                }
                println!("{} 已停止 {}", "✓".green(), record.domain);
                Ok(())
            }
            None => bail!("未找到隧道: {domain}"),
        }
    }

    async fn remove_tunnel(&self, domain: &str) -> Result<()> {
        let record = self
            .state
            .find(domain)?
            .ok_or_else(|| anyhow!("未找到隧道: {domain}"))?;

        let _ = self.state.stop_process(domain)?;
        let token = self
            .credentials
            .load()?
            .or_else(|| env::var("PUNCH_TOKEN").ok())
            .ok_or_else(|| anyhow!("删除远端隧道需要已登录的 Cloudflare 凭证"))?;

        let cf = CloudflareClient::new(token)?;
        if let Some(record_id) = &record.dns_record_id {
            cf.delete_dns_record(&record.zone_id, record_id).await?;
        }
        cf.delete_tunnel(&record.account_id, &record.tunnel_id)
            .await?;
        let _ = self.state.remove(domain)?;

        let log_path = PathBuf::from(record.log_file);
        if log_path.exists() {
            let _ = std::fs::remove_file(log_path);
        }

        println!(
            "{} 已删除 {}，远端资源和本地状态已清理",
            "✓".green(),
            domain
        );
        Ok(())
    }

    async fn show_logs(&self, domain: Option<&str>, follow: bool, lines: usize) -> Result<()> {
        let record = match domain {
            Some(domain) => self.state.find(domain)?,
            None => self.state.recent()?,
        }
        .ok_or_else(|| anyhow!("没有可用日志，请先启动一个隧道"))?;

        let path = PathBuf::from(record.log_file);
        if follow {
            follow_log_file(&path, lines).await
        } else {
            tail_log_file(&path, lines).await
        }
    }

    async fn doctor(&self) -> Result<()> {
        println!("PUNCH DOCTOR");
        println!("────────────");
        println!("home: {}", self.dirs.home().display());

        match Cloudflared::detect() {
            Ok(client) => println!("{} cloudflared: {}", "✓".green(), client.version()?),
            Err(error) => println!("{} cloudflared: {error:#}", "✗".red()),
        }

        match self
            .credentials
            .load()?
            .or_else(|| env::var("PUNCH_TOKEN").ok())
        {
            Some(token) => {
                let cf = CloudflareClient::new(token)?;
                match cf.verify_token().await {
                    Ok(result) if result.status == "active" => {
                        println!("{} token: active", "✓".green());
                        self.doctor_probe_cloudflare(&cf).await?;
                    }
                    Ok(result) => println!("{} token: {}", "⚠".yellow(), result.status),
                    Err(error) => println!("{} token: {error:#}", "✗".red()),
                }
            }
            None => println!("{} token: 未登录", "⚠".yellow()),
        }

        let tunnels = self.state.refresh_statuses()?;
        println!(
            "{} 已登记隧道: {}",
            "✓".green(),
            tunnels.len().to_string().bold()
        );

        for tunnel in tunnels {
            println!(
                "  - {} [{}] {}",
                tunnel.domain,
                tunnel.status,
                tunnel.local_target()
            );
        }
        Ok(())
    }

    async fn doctor_probe_cloudflare(&self, cf: &CloudflareClient) -> Result<()> {
        match cf.list_zones(5).await {
            Ok(zones) if zones.is_empty() => {
                println!("{} zone 读取: 没有可见 zone", "⚠".yellow());
            }
            Ok(zones) => {
                println!(
                    "{} zone 读取: {} 个，可见示例: {}",
                    "✓".green(),
                    zones.len(),
                    zones
                        .iter()
                        .map(|zone| zone.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                let zone = &zones[0];
                match cf.validate_tunnel_access(&zone.account.id).await {
                    Ok(()) => println!(
                        "{} tunnel 权限: 可访问 account {}",
                        "✓".green(),
                        zone.account.id
                    ),
                    Err(error) => println!("{} tunnel 权限: {error:#}", "✗".red()),
                }
            }
            Err(error) => println!("{} zone 读取: {error:#}", "✗".red()),
        }

        Ok(())
    }

    async fn up(
        &self,
        file: PathBuf,
        selected_protocol: Option<LocalProtocol>,
        insecure: bool,
    ) -> Result<()> {
        let config = PunchConfig::load(&file)?;
        if config.tunnels.is_empty() {
            bail!("配置文件中没有 tunnels 项");
        }

        for tunnel in config.tunnels {
            let protocol = match tunnel.https {
                Some(true) => Some(LocalProtocol::Https),
                _ => selected_protocol,
            };

            let options = LaunchOptions {
                protocol,
                insecure,
                detach: true,
                export: false,
                share: None,
            };
            let target = format!("{}:{}", tunnel.domain, tunnel.port);
            self.launch(&target, options).await?;
        }
        Ok(())
    }

    async fn share(
        &self,
        target: String,
        expires: &str,
        password: &str,
        selected_protocol: Option<LocalProtocol>,
        insecure: bool,
    ) -> Result<()> {
        let expires_at = parse_expiry(expires)?;
        let password_hint = "*".repeat(password.chars().count().min(8));
        let options = LaunchOptions {
            protocol: selected_protocol,
            insecure,
            detach: true,
            export: false,
            share: Some(ShareSettings {
                expires_at,
                password_hint,
            }),
        };
        self.launch(&target, options).await
    }

    async fn load_or_prompt_token(&self) -> Result<String> {
        if let Some(token) = self
            .credentials
            .load()?
            .or_else(|| env::var("PUNCH_TOKEN").ok())
        {
            return Ok(token);
        }

        println!("{}", "⚠ 未找到凭证，需要登录 Cloudflare".yellow());
        println!("步骤 1: 访问 https://dash.cloudflare.com/profile/api-tokens");
        println!("步骤 2: 创建令牌，权限: Zone:Read, DNS:Edit, Cloudflare Tunnel:Edit");
        println!("步骤 3: 粘贴令牌（输入已隐藏）:");
        let token = self.prompt_token()?;
        self.auth(Some(token.clone())).await?;
        Ok(token)
    }

    fn prompt_token(&self) -> Result<String> {
        print!("> ");
        io::stdout().flush().context("刷新终端失败")?;
        let token = rpassword::read_password().context("读取令牌失败")?;
        if token.trim().is_empty() {
            bail!("令牌不能为空");
        }
        Ok(token.trim().to_string())
    }

    async fn ensure_port_open(&self, port: u16) -> Result<()> {
        timeout(
            Duration::from_secs(2),
            TcpStream::connect(("127.0.0.1", port)),
        )
        .await
        .with_context(|| format!("✗ 端口 {port} 连接超时"))?
        .with_context(|| format!("✗ 端口 {port} 未监听，无法建立本地映射"))?;
        Ok(())
    }

    async fn detect_protocol(&self, port: u16, insecure: bool) -> Result<LocalProtocol> {
        self.ensure_port_open(port).await?;

        let https_client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .timeout(Duration::from_secs(2))
            .build()?;
        if https_client
            .get(format!("https://127.0.0.1:{port}"))
            .send()
            .await
            .is_ok()
        {
            if insecure {
                println!("{} 检测到本地 HTTPS，已启用 noTLSVerify", "✓".green());
            }
            return Ok(LocalProtocol::Https);
        }

        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()?;
        if http_client
            .get(format!("http://127.0.0.1:{port}"))
            .send()
            .await
            .is_ok()
        {
            return Ok(LocalProtocol::Http);
        }

        Ok(LocalProtocol::Http)
    }

    fn print_launch_success(&self, record: &TunnelRecord) {
        println!();
        println!("{} 服务上线: {}", "➜".green(), record.public_url().bold());
        println!("  本地映射: localhost:{}", record.local_port);
        println!("  日志文件: {}", record.log_file);
        println!();
    }
}

fn tunnel_name_for(domain: &str) -> String {
    let safe = domain.replace(|c: char| !c.is_ascii_alphanumeric(), "-");
    let mut name = format!("punch-{safe}");
    if name.len() > 58 {
        name.truncate(58);
    }
    name
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn format_runtime(started_at: DateTime<Utc>) -> String {
    let duration = Utc::now() - started_at;
    let seconds = duration.num_seconds().max(0);
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    if hours > 0 {
        format!("{hours}h{minutes:02}m")
    } else {
        format!("{minutes}m")
    }
}

fn parse_expiry(value: &str) -> Result<DateTime<Utc>> {
    let trimmed = value.trim();
    if trimmed.len() < 2 {
        bail!("无效过期时间: {value}");
    }
    let (number, suffix) = trimmed.split_at(trimmed.len() - 1);
    let amount: i64 = number
        .parse()
        .with_context(|| format!("无效过期时间: {value}"))?;
    let delta = match suffix {
        "m" => chrono::Duration::minutes(amount),
        "h" => chrono::Duration::hours(amount),
        "d" => chrono::Duration::days(amount),
        _ => bail!("无效过期时间单位: {suffix}，支持 m/h/d"),
    };
    Ok(Utc::now() + delta)
}
