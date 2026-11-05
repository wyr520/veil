# 部署

## 一、最小可用

```bash
REPO=<你的用户名>/veil
curl -fsSL https://raw.githubusercontent.com/$REPO/main/deploy/install.sh \
  | sudo bash -s -- --repo $REPO --domain node.example.com --email me@example.com
```

装完检查（三项都要看）：

```bash
systemctl status veil                  # active (running) + enabled
systemctl is-enabled veil              # enabled  → 开机自启
journalctl -u veil -n 30 --no-pager    # 有没有 bind 失败之类的报错
veil check -c /etc/veil/veil.toml
```

崩溃自动重启是靠这三行配置，不要删：

```ini
[Unit]
StartLimitIntervalSec=0     # 不限制重启次数
[Service]
Restart=always
RestartSec=3
```

验证它真的会重启：

```bash
sudo pkill -9 -f 'veil run' && sleep 4 && systemctl is-active veil   # 应输出 active
```

卸载：

```bash
REPO=<你的用户名>/veil
curl -fsSL https://raw.githubusercontent.com/$REPO/main/deploy/install.sh \
  | sudo bash -s -- --uninstall            # 交互确认，输 yes
# 常用变体：--uninstall --keep-config（保留账号/证书） / --uninstall --yes（不问）
```

本机也可以直接 `sudo veil uninstall`，效果一样。

### 离线 / 内网机器

```bash
# 在一台有网的机器上编译（或从 Release 下载），然后：
scp veil root@server:/root/veil
ssh root@server 'VEIL_BIN=/root/veil bash -s -- --domain node.example.com' < deploy/install.sh
```

## 二、DNS 与证书

1. 给域名加一条 A（或 AAAA）记录指向服务器公网 IP；
2. 等解析生效：`dig +short node.example.com`；
3. 自签证书只能给测试用，正式环境换 Let's Encrypt：

```bash
apt install -y certbot
systemctl stop veil                       # 443 端口先让出来
certbot certonly --standalone -d node.example.com
systemctl start veil
```

然后改 `/etc/veil/veil.toml`：

```toml
[server]
cert = "/etc/letsencrypt/live/node.example.com/fullchain.pem"
key  = "/etc/letsencrypt/live/node.example.com/privkey.pem"
cert_mode = "file"
insecure = false
```

```bash
systemctl restart veil
veil link            # 链接会自动去掉 allowInsecure=1
```

`veil` 以 `veil` 用户运行，需要能读 Let's Encrypt 目录：

```bash
setfacl -R -m u:veil:rX /etc/letsencrypt/live /etc/letsencrypt/archive
```

或者更省事：用一个 deploy hook 把证书复制到 `/etc/veil/cert/` 并 chown。

## 三、防火墙

最少放开这些：

| 端口 | 协议 | 用途 |
|---|---|---|
| 443/tcp | TLS | `vless-tls` |
| 8443/tcp | TLS+WS | `vless-ws`（可放到 CDN 后面） |
| 8444/tcp | TLS | `trojan-tls` |
| 8445/tcp | TLS+WS | `vmess-ws` |
| 8446/**udp** | QUIC | `hysteria2` |

```bash
# ufw
ufw allow 443/tcp && ufw allow 8443:8445/tcp && ufw allow 8446/udp

# firewalld
firewall-cmd --permanent --add-port=443/tcp --add-port=8443-8445/tcp --add-port=8446/udp
firewall-cmd --reload
```

云厂商的安全组要**单独**再放一次，很多人卡在这里。

> Hysteria2 走 UDP。部分机房/UDP 被限速或封锁时，把 `hysteria2` 入站的 `enable` 改成 `false` 即可。

## 四、管理 API 对外

API 默认只监听 `127.0.0.1:8080`。要把订阅链接发给别人，用 nginx 反代：

```nginx
server {
    listen 443 ssl http2;
    server_name api.example.com;

    ssl_certificate     /etc/letsencrypt/live/api.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/api.example.com/privkey.pem;

    # 订阅：允许公开访问（token 本身就是凭据）
    location /sub/ {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        add_header Cache-Control "no-store";
    }

    # 管理接口：加一层 basic auth
    location /api/ {
        auth_basic "veil";
        auth_basic_user_file /etc/nginx/.htpasswd;
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Authorization $http_authorization;
    }

    location / { return 404; }
}
```

```bash
apt install -y nginx apache2-utils
htpasswd -c /etc/nginx/.htpasswd admin
nginx -t && systemctl reload nginx
```

注意 `proxy_set_header Authorization` 会把 basic auth 的头部透传成 Bearer 之外的东西，
所以管理接口建议直接走 SSH 隧道，别暴露到公网：

```bash
ssh -L 8080:127.0.0.1:8080 root@node.example.com
# 然后本地访问 http://127.0.0.1:8080
```

## 五、WebSocket 走 CDN

Cloudflare 之类的 CDN 只回源 HTTP(S)，所以要给 WS 入站配一个 **HTTP 服务端**。
最简单的方式是让 nginx 监听 80/443 并把 WS 路径反代到 veil：

```nginx
server {
    listen 443 ssl http2;
    server_name cdn.example.com;
    ssl_certificate     /etc/letsencrypt/live/cdn.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/cdn.example.com/privkey.pem;

    location /veil {
        if ($http_upgrade != "websocket") { return 404; }
        proxy_pass http://127.0.0.1:8443;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;
    }
    location / { root /var/www/html; }
}
```

然后把 `vless-ws` 入站的 `listen` 改成 `127.0.0.1:8443`，只让 nginx 访问：

```toml
[[inbounds]]
tag = "vless-ws"
protocol = "vless"
listen = "127.0.0.1:8443"
transport = "ws"
path = "/veil"
```

⚠️ 此时 `transport = "ws"` 但 TLS 由 nginx 终结，veil 前面只剩明文 WS。
把该入站看作「已在 CDN 后」的私有链路，不要再暴露到公网。

## 六、运维

```bash
veil user add alice                       # 加人
veil user rotate alice                    # 出问题就轮换凭据
veil user enable alice false              # 临时停用
veil link --user alice                    # 重新打印链接
curl -s 127.0.0.1:8080/api/stats | jq     # 流量
```

**备份**只有两个东西：

```bash
tar czf veil-backup.tgz /etc/veil /var/lib/veil
```

恢复时解压 + `systemctl restart veil` 即可，配置里带全部凭据，注意文件权限（`chmod 600`）。

**升级**：

```bash
# 二进制替换
systemctl stop veil
install -m 0755 veil /usr/local/bin/veil
systemctl start veil
```

**卸载**：

```bash
systemctl disable --now veil
rm -f /etc/systemd/system/veil.service /usr/local/bin/veil
rm -rf /etc/veil /var/lib/veil /etc/sysctl.d/99-veil.conf
systemctl daemon-reload
```

## 七、排障清单

| 现象 | 检查 |
|---|---|
| 服务起不来 | `journalctl -u veil -n 50`，多半是 443 被 nginx/caddy 占了 |
| 客户端 TLS 握手失败 | 域名没解析对，或证书 SNI 不匹配；先 `openssl s_client -connect host:443 -servername host` |
| 能连上但不通 | `veil check` 看监听地址是不是 `0.0.0.0`；`curl -s 127.0.0.1:8080/api/health` |
| 只有某个协议不通 | 看 `journalctl` 里该 inbound tag 的日志，debug 级别：`log.level = "debug"` |
| Hysteria2 连不上 | 确认放的是 **UDP** 8446；`ss -ulnp | grep 8446` |
| 订阅拉不到 | API 绑在 127.0.0.1，需要反代；或直接用 `veil link --sub` |
| 速度慢 | 开 BBR（脚本已写）、换端口、或改用 `hysteria2`（UDP 抗丢包） |
