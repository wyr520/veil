# 协议实现说明

四个协议在 `src/proto/` 下都是**双端实现**：同一份编解码既给服务端用，也给 `veil client` 用。
每个模块末尾都有单元测试（`cargo test`，共 21 个），另外有一套真实互通测试（`scripts/interop.py`）。

---

## VLESS（`src/proto/vless.rs`）

```
请求：ver(1)=0 | uuid(16) | addon_len(1) | addons | cmd(1) | port(2BE) | atyp(1) | addr
响应：ver(1)=0 | addon_len(1) | addons
UDP ：cmd=2，随后每帧 len(2) | port(2) | atyp | addr | payload
```

* 注意字段顺序是 **port 在 atyp 之前**，和 SOCKS5 的 `atyp|addr|port` 相反。写反了服务端会报
  `unknown address type 97`（97 正是域名首字母 `a`）。
* 本身不加密，安全性完全来自 TLS。
* 不带 `flow`（`xtls-rprx-vision`）。vision 需要 TLS record 直通与 padding 混淆，属于 Xray 私有优化；
  不写 `flow` 时与所有客户端兼容。

## Trojan（`src/proto/trojan.rs`）

```
验证：sha224(password) 的 56 字节 hex | CRLF
请求：cmd(1) | atyp(1) | addr | port(2BE) | CRLF | payload...
UDP ：cmd=3，之后每帧 atyp | addr | port | len(2BE) | CRLF | payload
```

* 密码用 SHA-224 摘要比对，常量时间比较。
* 服务端先读 56 字节摘要，再遍历启用用户匹配，因此单端口支持多用户。
* 认证失败直接断开、不回任何字节，与 trojan-gfw 行为一致。

## VMess / AEAD（`src/proto/vmess.rs`）

```
cmdKey  = MD5(uuid(16) || "c48619fe-8f02-49e0-b9e9-edf763e17e21")

请求：auth_id(16) | enc(len)(2+16) | enc(header)(n+16)
header = ver(1) | body_iv(16) | body_key(16) | resp_v(1) | option(1)
         | pad_len(1) | pad | cmd(1) | port(2) | atyp | addr

body 块 = len(2) | ciphertext(len) | tag(16)      AAD = 那 2 字节 len
nonce  = counter(2 BE) || body_iv[2..12]
响应 key/iv = SHA256(请求 key/iv)[..16]，计数器归零
```

踩过的两个坑，都写进代码注释了：

1. **KDF 不是普通 HMAC。** V2Ray 的 `KDF` 是
   `HMAC-SHA256(key = HMAC-SHA256("VMess AEAD KDF", ""), msg = cmdKey || path...)`。
   用普通 HMAC 写出来的实现两端自洽、但和任何真实客户端都不通。
2. **AES-GCM 的 nonce 只能是 12 字节**，而 `KDF16` 输出 16 字节，必须取前 12 字节。

服务端不知道用户是谁，只能拿每个用户的 `cmdKey` 去试解长度字段；AEAD 校验通过即命中用户。
用户量几十的量级开销可忽略，上千用户建议按 `auth_id` 前缀分桶。

## Hysteria2（`src/proto/hysteria2.rs`）

一条 QUIC 连接承载三样东西：

```
控制流（第一条 bidi 流）： "HY2" + ver(1) + len(2) + password   →  "OK"
每条 TCP   ：开一条 bidi 流，先写 atyp | addr | port，随后裸字节双向直通
UDP        ：QUIC datagram， session(4LE) | atyp | addr | port | payload
             服务端回包 session(4LE) | payload
```

ALPN 固定 `h3`，握手特征与普通 HTTP/3 一致；`max_idle_timeout=90s`、datagram 缓冲 4MB，弱网下不易被抖动杀掉。

---

## 互通矩阵（实测）

`python scripts/interop.py` 会拉起真实服务端 + Xray-core 客户端 + 本地 HTTP 目标，逐项跑通后再报告：

| 用例 | 结果 | 说明 |
|---|---|---|
| VLESS + TLS ↔ Xray 26.7 | ✅ | |
| VLESS + WS ↔ Xray 26.7 | ✅ | |
| Trojan + TLS ↔ Xray 26.7 | ✅ | |
| VMess + WS ↔ Xray 26.7 | ❌ | 见下 |
| VLESS / Trojan / VMess / Hysteria2 ↔ veil client | ✅ | 全绿 |
| 管理 API、订阅 | ✅ | |

### VMess 与 Xray 的差异（未解决）

用 `veil` 自己两端互通完全正常，但 Xray 26.x 发来的请求头用我们的 `cmdKey`
解不开（服务端日志 `vmess: no user matches this handshake`，同时可以在 trace 级别抓到头 34 字节）。

已经排除的假设：`cmdKey` 的 MD5 输入、四种 KDF 变体（普通 HMAC / 双层 HMAC / 以 salt 为 HMAC key /
把 salt 拼进消息）、是否把 `auth_id` 拼进 path、12 与 16 字节 nonce、有无 AAD。
也就是说差异不在密钥派生，而在**头的整体布局**（大概率前面多了一个版本字节，或者长度字段不是
`2 密文 + 16 tag`）。要定位需要抓一份更长的字节做偏移扫描。

**生产建议：用 VLESS。** 它更简单、更快，也是 Xray 官方推荐替代 VMess 的协议。

### Hysteria2 与官方客户端的差异

官方 Hysteria2 的认证是"QUIC 上跑一个 HTTP/3 `POST /auth`，带 `Hysteria-Auth` 头"，
后面还有 `Hysteria-CC-RX`（要求 Brutal 拥塞控制）和 padding 协商。要做到字节级兼容，需要引入
`h3`/`h3-quinn` 实现 QPACK 头部编解码，以及 Brutal 拥塞控制（quinn 只提供 Cubic/BBR）。

当前用的是**自有极简控制流**，所以：

| 组合 | 结果 |
|---|---|
| veil 服务端 ↔ veil app / veil client | ✅ |
| veil 服务端 ↔ 官方 hysteria2 / sing-box | ❌ 认证阶段失败 |

需要官方客户端时，两条路：

1. 服务端并行跑一个官方 `hysteria` 进程，用 `outbound.mode = "socks"` 把它当上游串起来；
2. 给 `hysteria2.rs` 加一个 `h3` 认证后端（`write_auth` / `read_auth` 已经独立成函数，替换即可）。

---

## 传输层

| transport | 实现 | 说明 |
|---|---|---|
| `raw` | 裸 TCP | 仅限内网 / 已在外层加密的场景 |
| `tls` | rustls 1.3 | **不设 ALPN**，见下 |
| `ws` | tokio-tungstenite | 路径不匹配返回 404；能过 CDN |
| `quic` | quinn | 仅 Hysteria2 |

**为什么服务端不设 ALPN**：rustls 在服务端配置了 ALPN 而客户端没发（或列表不重叠）时会直接中止握手。
Xray、v2rayN、Shadowrocket 默认都不发 ALPN，于是握手全挂。伪装收益远小于兼容性损失，所以 ALPN 留空。

**WebSocket 的写语义**：`poll_write` 必须立即发一帧并 flush。早期版本只写进缓冲区，
配合 `write_all`（不 flush）导致小请求永远躺在内存里，隧道看起来"连上了但没数据"。

## 证书

`veil init` 生成的是 **CA + 叶证书** 两级结构：

```
/etc/veil/cert/ca.pem          自签 CA（导入系统信任库即可让浏览器/严格客户端信任）
/etc/veil/cert/fullchain.pem   叶证书 + CA
/etc/veil/cert/privkey.pem     叶证书私钥（0600）
```

用 CA:false 的单张自签证书时，Go 的 crypto/x509 会直接拒绝（`certificate signed by unknown authority`）。
两级结构 + `pinnedPeerCertSha256` 两条路都能让 Xray 26.x 这种"移除了 allowInsecure"的客户端连上：

```bash
veil cert show      # 打印 sha256 pin 和 ca.pem 路径
```

## 已实现 / 未实现

| 能力 | 状态 |
|---|---|
| VLESS TCP / UDP | ✅ |
| Trojan TCP / UDP | ✅ |
| VMess AEAD TCP（自身互通） | ✅ |
| VMess ↔ Xray 互通 | ❌ 见上 |
| VMess UDP / MUX | ❌ 显式报错 |
| Hysteria2（自有控制流） | ✅ |
| Hysteria2 ↔ 官方客户端 | ❌ 见上 |
| XTLS Vision | ❌ 不带 flow |
| gRPC / mKCP / HTTPUpgrade | ❌ |
| 多用户、流量统计 | ✅ |
| 出站链式代理（socks） | ✅ |
| 订阅、分享链接、二维码 | ✅ |

## 已知取舍

* **VLESS UDP / VMess UDP** 的帧格式按 Xray 文档实现，但未与其他客户端做过 UDP 互通验证；
  日常浏览走 TCP 不受影响。
* **配额字段**只记录不强制：`quota_bytes` 目前不会触发断连。
* **热重载**没有做：改完配置 `systemctl restart veil`，比实现 SIGHUP 热加载更不容易出错。
