# 手机端（Flutter）

## 构建

仓库里的 `app/` 只有 `lib/`、`pubspec.yaml` 和 `analysis_options.yaml`。**平台目录需要自己生成一次**，
这样不会把 Gradle / Xcode 的模板文件塞进版本库：

```bash
cd app
flutter create --project-name veil_app --org com.example --platforms android,ios .
flutter pub get
flutter run                # 调试
flutter build apk --release --split-per-abi   # 产物在 build/app/outputs/flutter-apk/
```

`flutter create .` 只会补齐 `android/`、`ios/`、`test/` 等目录，不会动 `lib/`。如果它提示是否覆盖文件，选 `n`。

Android 需要的最低配置（`android/app/build.gradle`）：

```gradle
android {
    defaultConfig {
        minSdkVersion 21
        targetSdkVersion 34
    }
}
```

网络权限在 Flutter 模板里默认就有（`<uses-permission android:name="android.permission.INTERNET"/>`）。

## 功能

| 页面 | 能力 |
|---|---|
| 首页 | 节点分组列表、汇总卡片（节点数 / 订阅数 / 可达数）、一键测全部延迟 |
| 添加 | 订阅链接、粘贴单条/多条分享链接、自建服务器（填域名 + 用户名，自动请求 `/api/nodes/<用户>`） |
| 详情 | 二维码、协议/传输/SNI/路径/UUID/密码、TCP 延迟、复制链接、删除 |
| 分组 | 下拉刷新订阅、整组删除 |

* 订阅体同时支持 **base64** 和 **明文链接列表**；
* `vmess://` 按 base64 JSON 解析，允许 URL-safe 字母表和缺失 padding；
* 链接和分组存在 `shared_preferences`，卸载即清空，不上传任何地方。

## 为什么不做"一键连"（重要）

一个真正的全局代理 App 需要三件东西：

1. **TUN 设备**：Android `VpnService.Builder().establish()` 返回一个 fd，iOS 是 `NEPacketTunnelProvider`；
2. **用户态 TCP/IP 栈**：TUN 给的是原始 IP 包，需要自己实现 TCP 状态机（或者引入 lwIP/smoltcp）；
3. **DNS 劫持**：把发往 53 端口的流量接住，否则域名解析会泄漏。

`veil` 的 Rust 核心目前提供的是 **SOCKS5 + HTTP 代理**（`src/client/`），没有 TUN 层。
所以 App 这一版定位是「节点管理器 + 分享工具」，全局接管交给现成客户端。这不是偷懒，是分层：

```
        ┌─────────────────────────────┐
        │  App（Dart）                │  节点管理、二维码、延迟
        └──────────────┬──────────────┘
                       │ 分享链接 / 订阅
        ┌──────────────▼──────────────┐
        │  veil 核心（Rust）           │  协议编解码 + SOCKS5/HTTP
        └──────────────┬──────────────┘
                       │ 可选：接 TUN
        ┌──────────────▼──────────────┐
        │  smoltcp tun2socks          │  ← 还没写
        └─────────────────────────────┘
```

## 要补上 TUN，怎么做

1. `Cargo.toml` 加 `smoltcp = { version = "0.11", default-features = false, features = ["std", "medium-ip", "proto-ipv4", "proto-ipv6", "socket-tcp", "socket-udp"] }`；
2. 在 `src/client/` 加 `tun.rs`：实现 `smoltcp::phy::Device`，`recv()` 从 TUN fd 读 IP 包，`send()` 写回；
3. `Interface::new` 配置 TUN 的地址（如 `10.7.0.1/24`）和默认路由；
4. 每个新 TCP 连接：`socket.listen(port)` → `socket.accept()` 后把它和 `Client::dial(dest)` 用 `relay()` 对接；
5. UDP 同理，把源地址映射到一条 `Client::dial` 出来的 UDP 会话；
6. Rust 侧暴露 `veil_tun_start(fd: i32, link: *const c_char)` 的 `extern "C"` 接口，用 `flutter_rust_bridge` 或 `dart:ffi` 调；
7. Android 侧 `VpnService` 只负责建 TUN、加路由、返回 fd，其余全在 Rust。

接口已经留好了：`Client::dial(&Destination) -> BoxedIo`，拿到的就是一条"已经过协议握手"的字节流，
tun2socks 只需要负责 IP 包 ↔ 字节流的转换。

## 已知限制

* 没有做 UDP associate（SOCKS5 的 `0x03` 命令回 `0x07` 拒绝），B 站/YouTube 的 QUIC 会落到 TCP；
* 只有 TCP 延迟，不是真实可用带宽；
* 订阅更新是手动的（下拉刷新），没有后台定时任务。
