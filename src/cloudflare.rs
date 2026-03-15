use anyhow::{Context, Result, anyhow, bail};
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct CloudflareClient {
    client: Client,
    token: String,
}

impl CloudflareClient {
    pub fn new(token: impl Into<String>) -> Result<Self> {
        let token = token.into();
        let client = Client::builder()
            .user_agent(format!("punch/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .context("创建 Cloudflare HTTP 客户端失败")?;
        Ok(Self { client, token })
    }

    pub async fn verify_token(&self) -> Result<TokenVerifyResult> {
        self.get("user/tokens/verify").await
    }

    pub async fn current_user_email(&self) -> Option<String> {
        match self.get::<UserProfile>("user").await {
            Ok(profile) => profile.email,
            Err(_) => None,
        }
    }

    pub async fn list_zones(&self, per_page: usize) -> Result<Vec<Zone>> {
        self.get(format!("zones?status=active&per_page={per_page}").as_str())
            .await
    }

    pub async fn validate_tunnel_access(&self, account_id: &str) -> Result<()> {
        let _: Vec<TunnelSummary> = self
            .get(format!("accounts/{account_id}/cfd_tunnel?per_page=1").as_str())
            .await?;
        Ok(())
    }

    pub async fn resolve_zone(&self, domain: &str) -> Result<Zone> {
        let parts = domain.split('.').collect::<Vec<_>>();
        for index in 0..parts.len().saturating_sub(1) {
            let candidate = parts[index..].join(".");
            let path = format!("zones?name={candidate}&status=active&per_page=1");
            let result: Vec<Zone> = self.get(path.as_str()).await?;
            if let Some(zone) = result.into_iter().next() {
                return Ok(zone);
            }
        }

        bail!(
            "✗ {domain} 未在 Cloudflare 托管\n  解决: 1) 登录 https://dash.cloudflare.com 2) 添加站点并修改 DNS"
        )
    }

    pub async fn ensure_tunnel(
        &self,
        account_id: &str,
        tunnel_name: &str,
    ) -> Result<TunnelDetails> {
        let list_path = format!(
            "accounts/{account_id}/cfd_tunnel?name={tunnel_name}&is_deleted=false&per_page=1"
        );
        let existing: Vec<TunnelSummary> = self.get(list_path.as_str()).await?;
        if let Some(tunnel) = existing.into_iter().next() {
            let token = self.get_tunnel_token(account_id, &tunnel.id).await?;
            return Ok(TunnelDetails {
                id: tunnel.id,
                name: tunnel.name,
                token,
            });
        }

        let payload = CreateTunnelRequest {
            name: tunnel_name.to_string(),
            config_src: "cloudflare".to_string(),
        };
        let result: CreateTunnelResult = self
            .post(
                format!("accounts/{account_id}/cfd_tunnel").as_str(),
                &payload,
            )
            .await?;

        let token = match result.token {
            Some(token) => token,
            None => self.get_tunnel_token(account_id, &result.id).await?,
        };

        Ok(TunnelDetails {
            id: result.id,
            name: result.name,
            token,
        })
    }

    pub async fn configure_tunnel(
        &self,
        account_id: &str,
        tunnel_id: &str,
        hostname: &str,
        service: &str,
        insecure: bool,
    ) -> Result<()> {
        let payload = TunnelConfigRequest {
            config: TunnelConfig {
                ingress: vec![
                    TunnelIngressRule {
                        hostname: Some(hostname.to_string()),
                        service: service.to_string(),
                        origin_request: Some(OriginRequest {
                            no_tls_verify: insecure,
                        }),
                    },
                    TunnelIngressRule {
                        hostname: None,
                        service: "http_status:404".to_string(),
                        origin_request: None,
                    },
                ],
            },
        };

        let path = format!("accounts/{account_id}/cfd_tunnel/{tunnel_id}/configurations");
        let _: serde_json::Value = self.put(path.as_str(), &payload).await?;
        Ok(())
    }

    pub async fn ensure_dns_record(
        &self,
        zone_id: &str,
        hostname: &str,
        target: &str,
    ) -> Result<DnsRecord> {
        let list_path =
            format!("zones/{zone_id}/dns_records?type=CNAME&name={hostname}&per_page=1");
        let existing: Vec<DnsRecord> = self.get(list_path.as_str()).await?;
        if let Some(record) = existing.into_iter().next() {
            let payload = DnsRecordRequest {
                record_type: "CNAME".to_string(),
                name: hostname.to_string(),
                content: target.to_string(),
                proxied: true,
                ttl: 1,
            };
            let path = format!("zones/{zone_id}/dns_records/{}", record.id);
            return self.put(path.as_str(), &payload).await;
        }

        let payload = DnsRecordRequest {
            record_type: "CNAME".to_string(),
            name: hostname.to_string(),
            content: target.to_string(),
            proxied: true,
            ttl: 1,
        };
        self.post(format!("zones/{zone_id}/dns_records").as_str(), &payload)
            .await
    }

    pub async fn delete_dns_record(&self, zone_id: &str, record_id: &str) -> Result<()> {
        let path = format!("zones/{zone_id}/dns_records/{record_id}");
        let _: serde_json::Value = self.delete(path.as_str()).await?;
        Ok(())
    }

    pub async fn delete_tunnel(&self, account_id: &str, tunnel_id: &str) -> Result<()> {
        let path = format!("accounts/{account_id}/cfd_tunnel/{tunnel_id}");
        let _: serde_json::Value = self.delete(path.as_str()).await?;
        Ok(())
    }

    async fn get_tunnel_token(&self, account_id: &str, tunnel_id: &str) -> Result<String> {
        let path = format!("accounts/{account_id}/cfd_tunnel/{tunnel_id}/token");
        match self.get::<String>(path.as_str()).await {
            Ok(token) => Ok(token),
            Err(_) => {
                let token_wrapper: TunnelTokenResponse = self.get(path.as_str()).await?;
                Ok(token_wrapper.token)
            }
        }
    }

    async fn get<T>(&self, path: &str) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        self.request(reqwest::Method::GET, path, Option::<&()>::None)
            .await
    }

    async fn post<T, B>(&self, path: &str, body: &B) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
        B: Serialize + ?Sized,
    {
        self.request(reqwest::Method::POST, path, Some(body)).await
    }

    async fn put<T, B>(&self, path: &str, body: &B) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
        B: Serialize + ?Sized,
    {
        self.request(reqwest::Method::PUT, path, Some(body)).await
    }

    async fn delete<T>(&self, path: &str) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        self.request(reqwest::Method::DELETE, path, Option::<&()>::None)
            .await
    }

    async fn request<T, B>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
        B: Serialize + ?Sized,
    {
        let url = format!("https://api.cloudflare.com/client/v4/{path}");
        let mut request = self
            .client
            .request(method, url)
            .bearer_auth(&self.token)
            .header("Content-Type", "application/json");

        if let Some(body) = body {
            request = request.json(body);
        }

        let response = request.send().await.context("Cloudflare API 请求失败")?;
        let status = response.status();
        let body = response
            .text()
            .await
            .context("读取 Cloudflare API 响应失败")?;
        let payload = serde_json::from_str::<ApiEnvelope<T>>(&body)
            .with_context(|| format!("解析 Cloudflare API 响应失败: {body}"))?;

        if payload.success {
            payload
                .result
                .ok_or_else(|| anyhow!("Cloudflare API 响应缺少 result 字段"))
        } else {
            let message = payload
                .errors
                .into_iter()
                .map(|error| match error.message {
                    Some(message) => message,
                    None => format!("Cloudflare 错误 {}", error.code),
                })
                .collect::<Vec<_>>()
                .join("; ");
            let hint = tunnel_permission_hint(&message);
            Err(anyhow!(
                "Cloudflare API 错误 ({}): {}{}",
                status,
                message,
                hint
            ))
        }
    }
}

#[derive(Debug, Deserialize)]
struct ApiEnvelope<T> {
    success: bool,
    #[serde(default)]
    errors: Vec<ApiError>,
    result: Option<T>,
}

#[derive(Debug, Deserialize)]
struct ApiError {
    code: u64,
    message: Option<String>,
}

fn tunnel_permission_hint(message: &str) -> &'static str {
    if message.contains("Authentication error") {
        "\n提示: 该 token 可以通过基础校验，但访问 Cloudflare Tunnel API 被拒绝。请确认已授予 `Cloudflare Tunnel:Edit` 或同等账户级权限。"
    } else {
        ""
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TokenVerifyResult {
    pub status: String,
}

#[derive(Debug, Deserialize)]
struct UserProfile {
    email: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Zone {
    pub id: String,
    pub name: String,
    pub account: ZoneAccount,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ZoneAccount {
    pub id: String,
}

#[derive(Debug, Clone)]
pub struct TunnelDetails {
    pub id: String,
    pub name: String,
    pub token: String,
}

#[derive(Debug, Deserialize)]
struct TunnelSummary {
    id: String,
    name: String,
}

#[derive(Debug, Serialize)]
struct CreateTunnelRequest {
    name: String,
    config_src: String,
}

#[derive(Debug, Deserialize)]
struct CreateTunnelResult {
    id: String,
    name: String,
    token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TunnelTokenResponse {
    token: String,
}

#[derive(Debug, Serialize)]
struct TunnelConfigRequest {
    config: TunnelConfig,
}

#[derive(Debug, Serialize)]
struct TunnelConfig {
    ingress: Vec<TunnelIngressRule>,
}

#[derive(Debug, Serialize)]
struct TunnelIngressRule {
    #[serde(skip_serializing_if = "Option::is_none")]
    hostname: Option<String>,
    service: String,
    #[serde(rename = "originRequest", skip_serializing_if = "Option::is_none")]
    origin_request: Option<OriginRequest>,
}

#[derive(Debug, Serialize)]
struct OriginRequest {
    #[serde(rename = "noTLSVerify")]
    no_tls_verify: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DnsRecord {
    pub id: String,
}

#[derive(Debug, Serialize)]
struct DnsRecordRequest {
    #[serde(rename = "type")]
    record_type: String,
    name: String,
    content: String,
    proxied: bool,
    ttl: u32,
}
