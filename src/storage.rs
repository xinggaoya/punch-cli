use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use keyring::Entry;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct PunchDirs {
    home: PathBuf,
}

impl PunchDirs {
    pub fn discover(home_override: Option<PathBuf>) -> Result<Self> {
        let home = match home_override.or_else(|| env::var_os("PUNCH_HOME").map(PathBuf::from)) {
            Some(path) => path,
            None => {
                let dirs = ProjectDirs::from("dev", "xinggao", "punch")
                    .context("无法推导 punch 配置目录，请设置 PUNCH_HOME")?;
                dirs.data_local_dir().to_path_buf()
            }
        };

        Ok(Self { home })
    }

    pub fn ensure(&self) -> Result<()> {
        fs::create_dir_all(self.logs_dir()).context("创建日志目录失败")?;
        fs::create_dir_all(self.cache_dir()).context("创建缓存目录失败")?;
        Ok(())
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.home.join("logs")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.home.join("cache")
    }

    pub fn state_file(&self) -> PathBuf {
        self.home.join("state.json")
    }

    pub fn fallback_token_file(&self) -> PathBuf {
        self.home.join("credentials.json")
    }

    pub fn log_file_for(&self, domain: &str) -> PathBuf {
        let safe = domain.replace(|c: char| !c.is_ascii_alphanumeric(), "-");
        self.logs_dir().join(format!("{safe}.log"))
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct FallbackCredential {
    token: String,
}

#[derive(Debug, Clone)]
pub struct CredentialStore {
    dirs: PunchDirs,
    service: &'static str,
    username: &'static str,
}

impl CredentialStore {
    pub fn new(dirs: PunchDirs) -> Self {
        Self {
            dirs,
            service: "dev.xinggao.punch",
            username: "cloudflare_api_token",
        }
    }

    pub fn load(&self) -> Result<Option<String>> {
        match Entry::new(self.service, self.username)?.get_password() {
            Ok(token) => return Ok(Some(token)),
            Err(keyring::Error::NoEntry) => {}
            Err(_) => {}
        }

        match fs::read_to_string(self.dirs.fallback_token_file()) {
            Ok(contents) => {
                let credential: FallbackCredential =
                    serde_json::from_str(&contents).context("解析回退凭证失败")?;
                Ok(Some(credential.token))
            }
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).context("读取回退凭证失败"),
        }
    }

    pub fn save(&self, token: &str) -> Result<CredentialBackend> {
        self.dirs.ensure()?;
        let keyring_saved = Entry::new(self.service, self.username)?
            .set_password(token)
            .is_ok();

        self.write_fallback_file(token)?;

        if keyring_saved {
            Ok(CredentialBackend::KeyringWithFallback)
        } else {
            Ok(CredentialBackend::FallbackFile)
        }
    }

    fn write_fallback_file(&self, token: &str) -> Result<()> {
        let payload = serde_json::to_string_pretty(&FallbackCredential {
            token: token.to_string(),
        })?;
        fs::write(self.dirs.fallback_token_file(), payload).context("写入本地凭证文件失败")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let permissions = fs::Permissions::from_mode(0o600);
            fs::set_permissions(self.dirs.fallback_token_file(), permissions)
                .context("设置本地凭证文件权限失败")?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub enum CredentialBackend {
    KeyringWithFallback,
    FallbackFile,
}
