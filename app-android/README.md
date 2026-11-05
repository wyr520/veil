# Veil for Android

单文件、零第三方依赖的 Android 客户端。装完就能：**导入订阅 → 看节点列表 → 复制/分享链接 → 测延迟 → 拉起已装的客户端**。

```
veil-debug.apk      41 KB，minSdk 21（Android 5.0），targetSdk 34
```

## 安装

```bash
# 方式一：adb
adb install -r veil-debug.apk

# 方式二：把 apk 传到手机，文件管理器点开安装
#         （需要在设置里允许"安装未知来源应用"）
```

首次打开是空的。把服务端打印的订阅地址贴进输入框，点"导入"：

```
https://node.example.com:8080/sub/xxxxxxxx
```

也支持直接粘贴单条 `vless://` / `trojan://` / `vmess://` / `hysteria2://` 链接。
订阅既可以是 base64 编码的，也可以是明文链接列表，两种都认。

## 能做什么

| 操作 | 说明 |
|---|---|
| 导入订阅 | 后台线程拉取，失败会在卡片上显示具体原因（HTTP 码 / 超时） |
| 节点列表 | 协议角标、传输方式、`host:port`、延迟色块；点击看详情 |
| 详情 | 完整分享链接（可长按选中）、SNI / PATH / UUID / 密码 |
| 复制 / 分享 | 走系统剪贴板和分享面板，粘到任何客户端里都能导入 |
| 测延迟 | 单个点"测延迟"，或**长按列表项**批量测 |
| 客户端打开 | 找已装的 v2rayNG / Hiddify / SagerNet / NekoBox，找到就复制链接并拉起 |

配置存在 `SharedPreferences`，杀进程重开还在，卸载即清空，不上传任何地方。

## 为什么没有"一键连"

Android 上真正的全局代理必须用 `VpnService` 建 TUN 设备，然后自己实现用户态 TCP/IP 栈
（或者塞 lwIP/smoltcp）。这个 APK 只有 41KB、零依赖，做不到那一步。

所以定位是**节点管理器 + 分发器**：它负责把节点管好，真正的隧道交给已经内置 TUN 的客户端。
Rust 服务端那边其实已经有完整的协议实现（`src/client/` 的 SOCKS5/HTTP 代理），
要做全内置只需给核心加一层 tun2socks 并用 JNI 接进来，路线写在 `../docs/APP.md`。

## 构建

不需要 Gradle、不需要 Android Studio、不联网。整条链就是 SDK 自带的那几个工具：

```bash
./build.sh          # aapt2 -> javac -> d8 -> zipalign -> apksigner
./test.sh           # 在 JVM 上跑解析器单测（不碰 Android 运行时）
```

依赖：

| 变量 | 默认值 |
|---|---|
| `ANDROID_SDK_ROOT` | `~/AndroidBuild/sdk` |
| `JAVA_HOME_DIR` | `~/AndroidBuild/jdk17` |

需要 `platforms/android-34/android.jar` 与 `build-tools/34.0.0`。
第一次构建会自动生成 `debug.jks`（口令 `android`），之后复用。

产物：`veil-debug.apk`。发布版：

```bash
VEIL_STORE_PASS='你的口令' ./build.sh release    # 用 release.jks 签
```

## 代码结构

```
src/com/veil/app/
├── MainActivity.java   界面（全代码构建视图，无 XML 布局）+ 交互
├── Node.java           分享链接解析（手写 URI 解析，不用 java.net.URI）
├── Sub.java            订阅拉取 / base64 解码
└── B64.java            容错 base64（URL-safe、缺 padding、空白）
tools/
├── icon.py             纯 Python 生成自适应前的多密度图标（不依赖 PIL）
└── jvmtest/ParseTest.java
```

`Node.parse` 刻意没用 `java.net.URI`：对 `vless://` 这种未注册 scheme，URI 可能把
authority 判成 registry-based，`getHost()` 直接返回 `null`。手写解析反而更稳，
而且 `+` 不会被当成空格（`URLDecoder` 的坑）。

## 已知限制

* 没有二维码（不想为它引依赖；需要的话扫详情页的链接文本，或用分享面板发到另一台设备）
* 订阅走 `HttpsURLConnection`，自签证书的订阅地址会失败 —— 订阅请挂真证书，或走内网 http
* 没有后台定时刷新订阅，进来点一下"导入"就是刷新
