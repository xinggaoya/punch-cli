# Punch 快速使用教程

这份文档按最短路径带你完成一次真实发布：

1. 安装 `cloudflared`
2. 登录 Cloudflare
3. 暴露本地服务
4. 查看状态和日志
5. 停止并删除 Tunnel

## 准备条件

开始之前，请确认：

- 你的域名已经接入 Cloudflare
- 你知道要使用的子域名，例如 `demo.example.com`
- 你的本地服务已经监听某个端口，例如 `8080`
- 你有一个 Cloudflare API Token

推荐 token 权限：

- `Zone:Read`
- `DNS:Edit`
- `Cloudflare Tunnel:Edit`

## 第 1 步：安装 `cloudflared`

这一步现在不是强制的。

如果系统里没有 `cloudflared`，`punch` 会在首次执行 `punch doctor` 或启动 Tunnel 时自动下载到：

```text
$PUNCH_HOME/cache/cloudflared/
```

### Linux

```bash
mkdir -p ~/.local/bin
curl -fL https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64 -o ~/.local/bin/cloudflared
chmod +x ~/.local/bin/cloudflared
export PATH="$HOME/.local/bin:$PATH"
```

### macOS

```bash
brew install cloudflared
```

### Windows

```powershell
winget install --id Cloudflare.cloudflared
```

检查版本：

```bash
cloudflared --version
```

## 第 2 步：构建 `punch`

### 方式 A：直接安装

如果仓库已经推到 GitHub，可以直接安装：

```bash
cargo install --git https://github.com/xinggaoya/punch-cli
```

### 方式 B：本地构建

在仓库根目录执行：

```bash
cargo build --release
```

也可以直接用开发模式运行：

```bash
cargo run -- --help
```

如果你想把 `punch` 安装到本机命令路径，可以手动复制：

```bash
cp target/release/punch ~/.local/bin/punch
```

## 第 3 步：登录 Cloudflare

### 方式 A：直接传 token

```bash
punch auth <TOKEN>
```

### 方式 B：交互式输入

```bash
punch auth
```

成功后，`punch` 会：

- 校验 token 是否有效
- 优先写入系统密钥环
- 同时写入本地回退文件，避免密钥环在某些环境中不可读

你可以立刻运行：

```bash
punch doctor
```

预期看到：

- `cloudflared` 正常
- `token: active`
- `zone 读取` 正常
- `tunnel 权限` 正常

如果 `doctor` 输出：

```text
Cloudflare API 错误 (403 Forbidden): Authentication error
```

通常表示你的 token 缺少 `Cloudflare Tunnel:Edit`。

## 第 4 步：启动你的本地服务

这里用一个最简单的例子。假设你要把当前目录通过 HTTP 暴露出去：

```bash
python3 -m http.server 8080 --bind 127.0.0.1
```

另开一个终端，先确认本地服务可访问：

```bash
curl -I http://127.0.0.1:8080
```

## 第 5 步：一条命令打洞

```bash
punch demo.example.com:8080 --http
```

如果本地服务就是 `8080`，也可以省略端口：

```bash
punch demo.example.com
```

如果本地是 HTTPS：

```bash
punch demo.example.com:8443 --https
```

如果是自签名证书：

```bash
punch demo.example.com:8443 --https --insecure
```

成功时你会看到类似输出：

```text
✓ Zone: example.com
✓ 隧道: punch-demo-example-com (uuid: ...)
✓ DNS: demo.example.com -> CNAME -> ....cfargotunnel.com

➜ 服务上线: https://demo.example.com
  本地映射: localhost:8080
  日志文件: ...

按 Ctrl+C 停止
```

前台模式停止后，这次临时会话会从本地状态中自动清理，不会继续留在 `punch ls` 里。

## 第 6 步：验证公网访问

```bash
curl https://demo.example.com
```

如果返回了你的本地页面内容，说明链路已经通了。

## 第 7 步：后台运行

如果你不想占用当前终端：

```bash
punch demo.example.com:8080 --http --detach
```

然后检查状态：

```bash
punch ls
```

## 第 8 步：查看日志

查看最近一次 Tunnel 日志：

```bash
punch logs
```

查看指定域名：

```bash
punch logs demo.example.com
```

持续跟随：

```bash
punch logs demo.example.com --follow
```

## 第 9 步：导出环境变量

如果你想在脚本里拿到公网地址：

```bash
eval "$(punch demo.example.com:8080 --export)"
echo "$PUNCH_URL"
```

当前会输出：

- `PUNCH_URL`
- `PUNCH_DOMAIN`
- `PUNCH_PORT`

## 第 10 步：批量启动

创建 `punch.yml`：

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

## 第 11 步：暴露指标

如果你要接入 Prometheus：

```bash
punch metrics --port 9090
```

然后访问：

```bash
curl http://127.0.0.1:9090/metrics
```

## 第 12 步：停止与清理

停止 Tunnel：

```bash
punch stop demo.example.com
```

只停止本地进程，不删除 Cloudflare 资源。

如果你想把远端 Tunnel 和 DNS 记录一起删除：

```bash
punch rm demo.example.com
```

这是清理测试环境时最应该执行的命令。

## 常见问题

### 1. `punch doctor` 显示 token 未登录

先重新执行：

```bash
punch auth
```

### 2. 报错 `端口未监听`

先确认本地服务已经起来：

```bash
curl -I http://127.0.0.1:8080
```

### 3. 报错 `未在 Cloudflare 托管`

说明域名对应的 Zone 不在当前 Cloudflare 账号里，或者还没接入 Cloudflare。

### 4. 报错 `Authentication error`

基础 token 校验通过，但账户级 Tunnel API 被拒绝。重新生成 token，并补齐：

- `Cloudflare Tunnel:Edit`

### 5. `share` 能直接做带密码的公网分享吗？

目前不能。`share` 只会记录分享元数据，还没有自动下发 Cloudflare Access 规则。

## 推荐操作顺序

首次使用时，建议严格按下面顺序：

```bash
punch auth
punch doctor
punch demo.example.com:8080
punch ls
punch logs demo.example.com
punch stop demo.example.com
punch rm demo.example.com
```

这是一条最完整、最安全的验证链路。
