use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use flate2::read::GzDecoder;
use reqwest::StatusCode;
use tar::Archive;
use tokio::fs::OpenOptions;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdout, Command};
use tokio::sync::Mutex;

use crate::storage::PunchDirs;

#[derive(Debug, Clone)]
pub struct Cloudflared {
    path: PathBuf,
}

impl Cloudflared {
    pub fn detect(dirs: &PunchDirs) -> Result<Self> {
        if let Ok(path) = which::which("cloudflared") {
            return Ok(Self { path });
        }

        let cached = dirs.cloudflared_binary_path();
        if cached.exists() {
            return Ok(Self { path: cached });
        }

        bail!(
            "✗ 未找到 cloudflared\n  解决: 运行需要 Tunnel 的命令时将自动下载，或手动安装后重试"
        )
    }

    pub async fn ensure_available(dirs: &PunchDirs) -> Result<Self> {
        if let Ok(client) = Self::detect(dirs) {
            return Ok(client);
        }

        dirs.ensure()?;
        let install_path = dirs.cloudflared_binary_path();
        if let Some(parent) = install_path.parent() {
            fs::create_dir_all(parent).context("创建 cloudflared 缓存目录失败")?;
        }

        let spec = download_spec_for_current_platform()?;
        println!(
            "→ 未检测到 cloudflared，开始自动下载到 {}",
            install_path.display()
        );
        download_to_path(&install_path, &spec).await?;
        println!("✓ cloudflared 已下载完成: {}", install_path.display());

        Ok(Self { path: install_path })
    }

    pub fn version(&self) -> Result<String> {
        let output = std::process::Command::new(&self.path)
            .arg("--version")
            .output()
            .context("读取 cloudflared 版本失败")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.trim().to_string())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn spawn_detached(&self, tunnel_token: &str, log_path: &Path) -> Result<u32> {
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent).context("创建日志目录失败")?;
        }

        let stdout = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .with_context(|| format!("打开日志文件失败: {}", log_path.display()))?;
        let stderr = stdout.try_clone().context("复制日志文件句柄失败")?;

        let mut command = std::process::Command::new(&self.path);
        command
            .args(["tunnel", "--no-autoupdate", "run", "--token", tunnel_token])
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;

            // 为后台进程创建独立进程组，避免跟随当前前台会话一起收信号。
            command.process_group(0);
        }

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;

            const DETACHED_PROCESS: u32 = 0x0000_0008;
            const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

            command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
        }

        let child = command.spawn().context("启动 cloudflared 失败")?;

        Ok(child.id())
    }

    pub fn spawn_foreground(&self, tunnel_token: &str) -> Result<Child> {
        Command::new(&self.path)
            .args(["tunnel", "--no-autoupdate", "run", "--token", tunnel_token])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("启动 cloudflared 失败")
    }
}

#[derive(Debug, Clone, Copy)]
enum DownloadKind {
    RawBinary,
    TarGz,
}

#[derive(Debug, Clone, Copy)]
struct DownloadSpec {
    url: &'static str,
    kind: DownloadKind,
}

fn download_spec_for_current_platform() -> Result<DownloadSpec> {
    let spec = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => DownloadSpec {
            url: "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64",
            kind: DownloadKind::RawBinary,
        },
        ("linux", "x86") => DownloadSpec {
            url: "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-386",
            kind: DownloadKind::RawBinary,
        },
        ("linux", "arm") => DownloadSpec {
            url: "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-arm",
            kind: DownloadKind::RawBinary,
        },
        ("linux", "aarch64") => DownloadSpec {
            url: "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-arm64",
            kind: DownloadKind::RawBinary,
        },
        ("macos", "x86_64") => DownloadSpec {
            url: "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-darwin-amd64.tgz",
            kind: DownloadKind::TarGz,
        },
        ("macos", "aarch64") => DownloadSpec {
            url: "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-darwin-arm64.tgz",
            kind: DownloadKind::TarGz,
        },
        ("windows", "x86_64") => DownloadSpec {
            url: "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe",
            kind: DownloadKind::RawBinary,
        },
        ("windows", "x86") => DownloadSpec {
            url: "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-386.exe",
            kind: DownloadKind::RawBinary,
        },
        (os, arch) => bail!("当前平台暂不支持自动下载 cloudflared: {os}/{arch}"),
    };

    Ok(spec)
}

async fn download_to_path(install_path: &Path, spec: &DownloadSpec) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent(format!("punch/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("创建下载客户端失败")?;

    let response = client
        .get(spec.url)
        .send()
        .await
        .context("下载 cloudflared 失败")?;
    let status = response.status();
    if status != StatusCode::OK {
        bail!("下载 cloudflared 失败，HTTP 状态码: {status}");
    }

    let payload = response.bytes().await.context("读取下载内容失败")?;
    let bytes = match spec.kind {
        DownloadKind::RawBinary => payload.to_vec(),
        DownloadKind::TarGz => extract_tgz_binary(payload.as_ref())?,
    };

    let temp_path = install_path.with_extension("download");
    fs::write(&temp_path, bytes).with_context(|| format!("写入 {} 失败", temp_path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let permissions = fs::Permissions::from_mode(0o755);
        fs::set_permissions(&temp_path, permissions)
            .with_context(|| format!("设置 {} 可执行权限失败", temp_path.display()))?;
    }

    fs::rename(&temp_path, install_path)
        .with_context(|| format!("安装 cloudflared 到 {} 失败", install_path.display()))?;
    Ok(())
}

fn extract_tgz_binary(archive_bytes: &[u8]) -> Result<Vec<u8>> {
    let decoder = GzDecoder::new(Cursor::new(archive_bytes));
    let mut archive = Archive::new(decoder);

    for entry in archive.entries().context("读取 tgz 内容失败")? {
        let mut entry = entry.context("读取 tgz 条目失败")?;
        if !entry.header().entry_type().is_file() {
            continue;
        }

        let path = entry.path().context("读取 tgz 条目路径失败")?;
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if !name.starts_with("cloudflared") {
            continue;
        }

        let mut buffer = Vec::new();
        entry
            .read_to_end(&mut buffer)
            .context("解压 cloudflared 二进制失败")?;
        if buffer.is_empty() {
            bail!("下载的 cloudflared 压缩包内容为空");
        }
        return Ok(buffer);
    }

    Err(anyhow!("压缩包中未找到 cloudflared 可执行文件"))
}

pub async fn pump_streams(
    stdout: ChildStdout,
    stderr: ChildStderr,
    log_path: PathBuf,
    to_stdout: bool,
) -> Result<()> {
    if let Some(parent) = log_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("创建日志目录失败")?;
    }

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .await
        .with_context(|| format!("打开日志文件失败: {}", log_path.display()))?;
    let shared = Arc::new(Mutex::new(file));

    let out_task = tokio::spawn(copy_stream(stdout, shared.clone(), to_stdout));
    let err_task = tokio::spawn(copy_stream(stderr, shared, to_stdout));

    let _ = out_task.await.context("处理 stdout 日志失败")??;
    let _ = err_task.await.context("处理 stderr 日志失败")??;
    Ok(())
}

async fn copy_stream<R>(reader: R, sink: Arc<Mutex<tokio::fs::File>>, to_stdout: bool) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    while let Some(line) = lines.next_line().await.context("读取日志流失败")? {
        let mut file = sink.lock().await;
        file.write_all(line.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;
        drop(file);

        if to_stdout {
            println!("{line}");
        }
    }

    Ok(())
}

pub async fn follow_log_file(path: &Path, lines: usize) -> Result<()> {
    let contents = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("读取日志文件失败: {}", path.display()))?;
    let selected = contents
        .lines()
        .rev()
        .take(lines)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    for line in selected {
        println!("{line}");
    }

    let mut seen = contents.len();
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let contents = tokio::fs::read_to_string(path).await?;
        if contents.len() < seen {
            seen = 0;
        }
        if contents.len() > seen {
            print!("{}", &contents[seen..]);
            seen = contents.len();
        }
    }
}

pub async fn tail_log_file(path: &Path, lines: usize) -> Result<()> {
    let contents = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("读取日志文件失败: {}", path.display()))?;
    let selected = contents
        .lines()
        .rev()
        .take(lines)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();
    if selected.is_empty() {
        bail!("日志为空: {}", path.display());
    }
    for line in selected {
        println!("{line}");
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use tempfile::tempdir;

    use super::Cloudflared;

    #[test]
    fn detached_process_keeps_running_after_spawn_returns() {
        let temp = tempdir().expect("temp dir should be created");
        let script_path = temp.path().join("cloudflared");
        let log_path = temp.path().join("cloudflared.log");

        fs::write(&script_path, "#!/bin/sh\nsleep 5\n").expect("script should be written");

        let mut permissions = fs::metadata(&script_path)
            .expect("metadata should be available")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script_path, permissions).expect("permissions should be set");

        let client = Cloudflared {
            path: script_path.clone(),
        };
        let pid = client
            .spawn_detached("fake-token", &log_path)
            .expect("detached process should start");

        assert!(
            std::path::Path::new(&format!("/proc/{pid}")).exists(),
            "detached process should still exist"
        );

        let _ = std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status();
    }
}
