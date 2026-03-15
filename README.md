# punch

三秒打洞，本地服务即刻上线。

`punch` 是一个基于 Cloudflare Tunnel 和 `cloudflared` 的 Rust CLI，用一条命令把本地服务映射到固定公网域名。

```bash
punch demo.example.com:8080
```

输出目标是：

```text
https://demo.example.com -> localhost:8080
```

## 当前状态

当前仓库已经实现并验证了以下能力：

- `punch <domain>[:port]`
- `punch auth`
- `punch ls`
- `punch stop`
- `punch rm`
- `punch logs`
- `punch doctor`
- `punch up`
- `punch metrics`
- `punch share`（实验性，当前只记录分享元数据）
- `punch --version`

已完成的真实链路验证：

- Cloudflare token 认证
- Zone 解析
- Tunnel 创建
- DNS CNAME 写入
- 前台模式启动 `cloudflared`
- 公网域名访问本地 HTTP 服务
- `ls / logs / stop / rm / metrics` 命令

## 设计目标

- 比 `cloudflared` 更省步骤
- 比传统 FRP 配置更轻
- 默认使用固定域名
- 自动处理 Tunnel 和 DNS 资源
- 提供本地状态管理、日志、诊断和指标导出

## 环境要求

- Rust 1.94 或更新版本
- 已安装 `cloudflared`
- 目标域名已接入 Cloudflare
- 一个具备以下权限的 Cloudflare API Token

推荐权限：

- `Zone:Read`
- `DNS:Edit`
- `Cloudflare Tunnel:Edit`

如果 token 基础校验能通过，但创建 Tunnel 时返回 `403 Authentication error`，通常表示缺少账户级 Tunnel 权限。

## 安装与构建

### 1. 直接通过 GitHub 安装

```bash
cargo install --git https://github.com/xinggaoya/punch-cli
```

如果你想安装指定分支：

```bash
cargo install --git https://github.com/xinggaoya/punch-cli --branch master
```

### 2. 从源码构建

```bash
cargo build --release
```

产物位置：

```text
target/release/punch
```

### 3. 安装 `cloudflared`

Linux 可直接下载官方发布版本：

```bash
mkdir -p ~/.local/bin
curl -fL https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64 -o ~/.local/bin/cloudflared
chmod +x ~/.local/bin/cloudflared
```

macOS 可使用 Homebrew：

```bash
brew install cloudflared
```

Windows 可使用：

```powershell
winget install --id Cloudflare.cloudflared
```

确认安装：

```bash
cloudflared --version
```

## 快速开始

详细版见 [docs/quickstart.md](docs/quickstart.md)。

### 1. 登录 Cloudflare

```bash
punch auth <TOKEN>
```

或交互式输入：

```bash
punch auth
```

认证成功后：

- 优先写入系统密钥环
- 同时写入本地回退文件，避免某些 Linux 会话里密钥环不可读

### 2. 暴露本地服务

假设本地服务运行在 `8080`：

```bash
punch demo.example.com:8080
```

如果不写端口，默认用 `8080`：

```bash
punch demo.example.com
```

如果明确知道本地是 HTTPS：

```bash
punch demo.example.com:8443 --https
```

前台模式下按 `Ctrl+C` 停止。

前台模式是临时会话：

- 运行中会写入本地状态，便于 `punch stop` / `punch logs`
- 收到 `Ctrl+C` 或进程退出后，会自动从本地状态里移除
- 只有后台 `--detach` 模式会长期保留在 `punch ls` 里

### 3. 后台启动

```bash
punch demo.example.com:8080 --detach
```

后台模式会为 `cloudflared` 创建独立进程组，并把日志重定向到本地文件，适合作为持续运行的托管方式。

如果你希望给 shell 注入公网地址：

```bash
eval "$(punch demo.example.com:8080 --export)"
echo "$PUNCH_URL"
```

### 4. 查看状态

```bash
punch ls
```

### 5. 查看日志

```bash
punch logs
punch logs demo.example.com
```

持续跟随：

```bash
punch logs demo.example.com --follow
```

### 6. 停止与删除

停止本地运行中的 Tunnel：

```bash
punch stop demo.example.com
```

删除远端 Tunnel 与 DNS：

```bash
punch rm demo.example.com
```

## 常用命令

| 命令 | 说明 |
| --- | --- |
| `punch <domain>[:port]` | 启动单个 Tunnel，默认端口 `8080` |
| `punch auth [token]` | 登录 Cloudflare |
| `punch ls` | 查看本地登记的 Tunnel |
| `punch stop <domain>` | 停止本地 Tunnel 进程 |
| `punch rm <domain>` | 删除远端 Tunnel 和 DNS，并清理本地状态 |
| `punch logs [domain]` | 查看日志，默认最近一个 |
| `punch doctor` | 检查 `cloudflared`、token、zone 和 tunnel 权限 |
| `punch up` | 从 `punch.yml` 批量启动 |
| `punch share` | 实验性分享模式 |
| `punch metrics --port 9090` | 暴露 Prometheus 指标 |
| `punch --version` | 显示 `punch` 和 `cloudflared` 版本 |

## 配置文件

支持通过 `punch.yml` 批量启动：

```yaml
tunnels:
  - domain: api.staging.example.com
    port: 3000
    https: true

  - domain: webhooks.dev.example.com
    port: 8080
```

启动：

```bash
punch up
```

指定文件：

```bash
punch up --file ./deploy/punch.yml
```

## Metrics

启动 Prometheus 指标端口：

```bash
punch metrics --port 9090
```

抓取结果示例：

```text
punch_active_tunnels 1
punch_total_tunnels 1
punch_tunnel_port{domain="demo.example.com",protocol="http"} 8080
punch_tunnel_up{domain="demo.example.com",protocol="http"} 1
```

## 排错

### `cloudflared` 未安装

现象：

```text
✗ 未找到 cloudflared
```

处理：

- 安装 `cloudflared`
- 确认其在 `PATH` 中可见

### token 已登录但无法创建 Tunnel

现象：

```text
Cloudflare API 错误 (403 Forbidden): Authentication error
```

处理：

- 重新创建 Cloudflare API Token
- 确保含有 `Cloudflare Tunnel:Edit`
- 再次运行 `punch doctor`

### 域名未托管在 Cloudflare

现象：

```text
<domain> 未在 Cloudflare 托管
```

处理：

- 先把站点接入 Cloudflare
- 确认 NS 已切换

### 本地端口没有监听

现象：

```text
✗ 端口 8080 未监听，无法建立本地映射
```

处理：

- 先启动本地服务
- 或切换到正确端口

## 已知限制

- `share` 目前只记录过期时间和密码提示，不会自动配置 Cloudflare Access 规则
- `--tcp` 依赖 Cloudflare 付费能力，当前仅保留接口
- 当前状态统计还没有接入真实流量计数
- 在某些受限宿主环境中，后台 detached 子进程可能被外层进程管理器回收；正常终端环境下不受这个限制

## 开发

运行测试：

```bash
cargo test
```

查看 CLI 帮助：

```bash
cargo run -- --help
```

## 文档

- [快速使用教程](docs/quickstart.md)
