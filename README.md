# Veil

多协议代理套件：**Rust 数据面 + 一条命令的服务端部署 + Flutter 手机端**。

支持 **VLESS / Trojan / VMess(AEAD) / Hysteria2** 四种入站，可叠加 **TLS / WebSocket+TLS / 裸 TCP / QUIC** 四种传输。

```
┌──────────────┐    TLS / WS+TLS / QUIC     ┌───────────────────────────┐
│  veil client │ ─────────────────────────► │  veil server              │
│  SOCKS5/HTTP │                            │  vless  trojan  vmess  hy2│
└──────────────┘                            └─────────────┬─────────────┘
       ▲                                                   │
       │ 订阅 / 分享链接                                     ▼
┌──────────────┐                                    目标网站 / 服务
│  veil app    │
└──────────────┘
```

---

## 互通性（实测，不是"应该可以"）

`python scripts/interop.py` 会拉起真实服务端、Xray-core 客户端和本地 HTTP 目标逐项验证：

| 用例 | 结果 |
|---|---|
| VLESS+TLS / VLESS+WS / Trojan+TLS ↔ **Xray 26.7** | ✅ 通过 |
| VLESS / Trojan / VMess / Hysteria2 ↔ **veil 自己的客户端** | ✅ 全部通过 |
| VMess+WS ↔ Xray 26.7 | ❌ 请求头布局仍有差异，详见 `docs/PROTOCOLS.md` |
| Hysteria2 ↔ 官方 hysteria / sing-box | ❌ 认证流程是自有的，详见 `docs/PROTOCOLS.md` |

结论：**主力用 VLESS**（Xray 官方也建议用它替代 VMess），需要抗封锁就叠 WebSocket+TLS；
`veil` 自己的客户端和 App 支持全部四种。为什么没能做到百分百兼容，文档里写清了原因和补齐路径。

---

## 1. 上传到 GitHub（只需要做一次）

仓库已经在本地 `git init` 并提交好了，只差推上去。二选一：

```bash
# A. 用 GitHub CLI（推荐，会自动建仓库）
cd veil
gh auth login                                    # 浏览器授权，一次性
gh repo create veil --public --source=. --push
git tag v0.1.0 && git push --tags                # 触发 Actions 编译 Linux 二进制

# B. 网页上先建好空仓库，再手动关联
cd veil
git remote add origin git@github.com:<你的用户名>/veil.git
git push -u origin main
git tag v0.1.0 && git push --tags
```

`v0.1.0` 这个 tag 会触发 `.github/workflows/release.yml`，在 GitHub 上编出
`veil-x86_64-unknown-linux-musl.tar.gz` 和 `aarch64` 版本挂到 Release 上（静态链接，任何发行版都能跑）。
**没有 release 也能装**：安装脚本检测不到二进制会自动在服务器上源码编译，只是多花几分钟。

## 2. 一条命令装好

```bash
REPO=<你的用户名>/veil
curl -fsSL https://raw.githubusercontent.com/$REPO/main/deploy/install.sh \
  | sudo bash -s -- --repo $REPO --domain node.example.com --email me@example.com
```

> `--repo` 是必须的：脚本被管道喂给 bash 时看不到自己的来源地址，得告诉它去哪儿取二进制。

| 步骤 | 说明 |
|---|---|
| 依赖 | 识别 apt/dnf/yum/apk/pacman/zypper，装 curl、ca-certificates、tar |
| 二进制 | `VEIL_BIN` 本地文件 → Release 下载 → 源码编译，三级回退 |
| 系统用户 | 创建 `veil` 用户，`/etc/veil`（配置+证书）、`/var/lib/veil`（状态） |
| 配置 | `veil init` 生成随机 UUID / 密码 / 订阅 token，签发 CA+叶证书 |
| **开机自启** | `systemctl enable veil`；无 systemd 时用 OpenRC `rc-update add` |
| **崩溃重启** | `Restart=always` + `RestartSec=3` + `StartLimitIntervalSec=0`（永远不放弃重试） |
| 端口冲突 | 443 被 nginx 之类占了会自动把 `vless-tls` 挪到 8443 并在输出里说明 |
| 调优 | BBR + fq + 32MB 缓冲区写入 `/etc/sysctl.d/99-veil.conf` |
| 输出 | 全部链接、admin token、订阅地址、下一步清单 |

验证：

```bash
systemctl status veil       # active (running)
journalctl -u veil -n 30    # 看有没有报错
veil check -c /etc/veil/veil.toml
```

常用变体：

```bash
# 用本地已编译好的二进制（离线机器）
VEIL_BIN=/root/veil sudo -E bash deploy/install.sh --domain node.example.com

# 强制源码编译（release 还没建好时也可用）
sudo bash deploy/install.sh --source --repo <你>/veil --domain node.example.com

# 国内网络：rustup 和 clone 都走镜像
sudo bash deploy/install.sh --source --mirror cn --repo <你>/veil --domain node.example.com

# 不想要 sysctl 调优
sudo bash deploy/install.sh --no-tune --repo <你>/veil --domain n.example.com
```

默认开五个入站：

| tag | 协议 | 传输 | 端口 |
|---|---|---|---|
| `vless-tls` | VLESS | TLS | 443 |
| `vless-ws` | VLESS | WebSocket + TLS | 8443 |
| `trojan-tls` | Trojan | TLS | 8444 |
| `vmess-ws` | VMess | WebSocket + TLS | 8445 |
| `hysteria2` | Hysteria2 | QUIC (UDP) | 8446 |

> 端口冲突时改 `/etc/veil/veil.toml` 里的 `listen`，然后 `systemctl restart veil`。

### 手动操作

```bash
veil init  -c /etc/veil/veil.toml --domain node.example.com
veil check -c /etc/veil/veil.toml       # 校验配置
veil run   -c /etc/veil/veil.toml       # 前台运行
veil user add bob                       # 加用户并打印他的全部链接
veil user list
veil user rotate bob                    # 轮换 UUID / 密码 / 订阅 token
veil link --user bob                    # 打印分享链接
veil link --sub                         # 打印 base64 订阅内容
veil cert show                          # 证书路径与模式
```

## 3. 一条命令卸载

```bash
REPO=<你的用户名>/veil
curl -fsSL https://raw.githubusercontent.com/$REPO/main/deploy/install.sh \
  | sudo bash -s -- --uninstall
```

会依次：停服务 → 取消开机自启 → 删 `veil.service` → 删 `/usr/local/bin/veil` →
删 `/var/lib/veil` → 删 `/etc/veil`（含凭据和证书）→ 删 sysctl 调优 → 删除 `veil` 用户。
交互确认，要输入 `yes`。

```bash
# 保留配置（只停服务删程序，账号和证书留着）
... | sudo bash -s -- --uninstall --keep-config

# 不问了，直接删
... | sudo bash -s -- --uninstall --yes

# 或者已经装了 veil 的话，本机直接执行
sudo veil uninstall
sudo veil uninstall --keep-config
```

---

## 4. 真实证书

`veil init` 生成的是 **CA + 叶证书** 两级自签结构。客户端要么跳过校验（旧客户端），要么用下面的 pin 方式。
生产环境换成真证书：

```bash
# 方式 A：certbot 签发后指向文件
sudo certbot certonly --standalone -d node.example.com
sudo sed -i 's#^cert = .*#cert = "/etc/letsencrypt/live/node.example.com/fullchain.pem"#' /etc/veil/veil.toml
sudo sed -i 's#^key = .*#key  = "/etc/letsencrypt/live/node.example.com/privkey.pem"#'  /etc/veil/veil.toml
sudo sed -i 's#^cert_mode = .*#cert_mode = "file"#' /etc/veil/veil.toml
sudo sed -i 's#^insecure = .*#insecure = false#' /etc/veil/veil.toml
sudo systemctl restart veil
```

`cert_mode = "file"` 之后 `veil` 不会覆盖你的证书。Let's Encrypt 的续期用 certbot 自带的 timer，续期后 `systemctl reload-or-restart veil` 即可。

### 还是自签证书时，怎么让 Xray 26 连上

Xray ≥ 25 删掉了 `allowInsecure`。两条合法路径：

```bash
veil cert show
# sha256 pin : a2ba7c54...
# CA bundle  : /etc/veil/cert/ca.pem
```

1. 客户端写 `"tlsSettings": { "pinnedPeerCertSha256": "<pin>" }`（十六进制）；
2. 或把 `ca.pem` 导入系统/应用信任库，客户端就正常校验证书链。

`veil init` 生成的是 **CA + 叶证书** 两级结构，所以第 2 条真的可用（单张 CA:false 自签证书会被
Go 的 crypto/x509 拒绝）。

---

## 5. 客户端（桌面 / 无 GUI 环境）

```bash
veil client -c /etc/veil/veil.toml --node vless-tls \
            --listen 127.0.0.1:1080 --http 127.0.0.1:8118
```

* `--listen`：本地 SOCKS5
* `--http`：本地 HTTP 代理（支持 `CONNECT` 与普通 HTTP）
* `--node`：用哪个入站当上游（省略则取第一个 enabled 的）

测延迟：

```bash
veil ping --node hysteria2 --target www.gstatic.com:80 --count 5
```

---

## 6. 手机端

**已经打好包，直接装：`app-android/veil-debug.apk`（41 KB）**

```bash
adb install -r app-android/veil-debug.apk
# 或者把 apk 传到手机，文件管理器点开（需允许"安装未知来源应用"）
```

打开 → 贴订阅地址 → 点"导入"。节点列表就有了：长按测延迟、点开看完整链接、
复制/分享、或者一键拉起已装的 v2rayNG / Hiddify。

它是**零第三方依赖**的纯 framework 实现（没有 Gradle、没有 AndroidX），所以只有 41 KB；
构建只要 SDK 自带的 `aapt2 + javac + d8 + apksigner`：

```bash
cd app-android && ./build.sh     # 产物 veil-debug.apk
cd app-android && ./test.sh      # 解析器单测（在 JVM 上跑，不碰 Android 运行时）
```

另外 `app/` 里还有一份 **Flutter 跨平台版**（Android + iOS，带二维码和分组管理），
需要自己装 Flutter SDK 才能编译：

```bash
cd app && flutter create . && flutter pub get && flutter run
```

> 两个客户端都定位在"节点管理 + 分发"：真正需要全局 TUN 接管时，把链接导入
> v2rayNG / Hiddify / sing-box 这类已内置 TUN 的客户端。为什么这么分层、以及要自己做
> TUN 该补什么，写在 [`docs/APP.md`](docs/APP.md)。

---

## 7. 管理 API

默认监听 `127.0.0.1:8080`，对外请用 nginx 反代并加一层认证。

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/health` | 状态、版本、运行时长 |
| GET | `/api/nodes/{user}` | 该用户的节点列表（含分享链接） |
| GET | `/sub/{token}` | base64 订阅（给客户端当订阅地址） |
| GET | `/api/stats` | 每个用户的上行/下行字节数 |
| GET | `/api/users` | 用户列表（需 `Authorization: Bearer <admin_token>`） |
| POST | `/api/users` | 新建用户，body `{"name":"bob"}`（需 admin token） |

```bash
curl -s http://127.0.0.1:8080/api/health | jq
curl -s -H "Authorization: Bearer $TOKEN" -X POST \
     -d '{"name":"carol"}' http://127.0.0.1:8080/api/users
```

---

## 8. 配置文件

`/etc/veil/veil.toml`：

```toml
[server]
name = "veil-node"
domain = "node.example.com"
cert = "/etc/veil/cert/fullchain.pem"
key = "/etc/veil/cert/privkey.pem"
cert_mode = "selfsigned"   # selfsigned | file | acme
insecure = true            # 客户端是否跳过证书校验（写进分享链接）

[outbound]
mode = "direct"            # direct | socks | block
socks = ""                 # mode = "socks" 时的上游，如 "127.0.0.1:1080"
blocked_ports = [25, 465, 587, 6667]
block_private = false
connect_timeout = 10

[api]
listen = "127.0.0.1:8080"
admin_token = "..."
sub_path = "/sub"
enable = true

[log]
level = "info"

[[inbounds]]
tag = "vless-tls"
protocol = "vless"
listen = "0.0.0.0:443"
transport = "tls"
path = "/veil"
sni = "node.example.com"

[[users]]
name = "default"
uuid = "……"
password = "……"
enable = true
sub_token = "……"
```

---

## 9. 目录结构

```
veil/
├── src/
│   ├── proto/        协议编解码（vless / trojan / vmess / hysteria2，双端实现）
│   ├── net/          TLS、WebSocket、QUIC、SOCKS5、转发
│   ├── server/       监听器、路由、出站、管理 API
│   ├── client/       本地 SOCKS5 / HTTP 代理
│   ├── config.rs     配置模型与校验
│   ├── share.rs      分享链接 / 订阅
│   └── crypto.rs     KDF、AES-GCM、SHA-224、HMAC
├── deploy/           install.sh、systemd unit
├── scripts/
│   └── interop.py    端到端互通测试（真起服务端 + Xray 客户端）
├── app-android/      Android 客户端 + 已签名 APK（零依赖，SDK 工具链直接构建）
├── app/              Flutter 跨平台客户端（Android/iOS）
└── docs/             PROTOCOLS.md（协议细节与互通矩阵）、DEPLOY.md、APP.md
```

## 10. 从源码构建

```bash
cargo build --release            # 产物 target/release/veil
cargo test                       # 协议编解码单元测试
cargo build --release --target x86_64-unknown-linux-musl   # 静态链接，方便分发
```

交叉编译到 Linux（在 Linux 上）：

```bash
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
cargo install cross
cross build --release --target aarch64-unknown-linux-musl
```

## 11. 许可

MIT。仅用于自建网络、远程办公、跨地域访问自有服务等合法用途；请遵守所在地区的法律法规与服务商条款。
