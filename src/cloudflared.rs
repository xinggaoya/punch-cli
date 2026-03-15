use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use tokio::fs::OpenOptions;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdout, Command};
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct Cloudflared {
    path: PathBuf,
}

impl Cloudflared {
    pub fn detect() -> Result<Self> {
        let path = which::which("cloudflared")
            .context("✗ 未找到 cloudflared\n  解决: 安装 cloudflared 后重试，或将其加入 PATH")?;
        Ok(Self { path })
    }

    pub fn version(&self) -> Result<String> {
        let output = std::process::Command::new(&self.path)
            .arg("--version")
            .output()
            .context("读取 cloudflared 版本失败")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.trim().to_string())
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

        let child = std::process::Command::new(&self.path)
            .args(["tunnel", "--no-autoupdate", "run", "--token", tunnel_token])
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .context("启动 cloudflared 失败")?;

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
