#!/usr/bin/env bash
# =============================================================================
#  Veil — one-command installer / uninstaller
#
#  Install:
#    curl -fsSL https://raw.githubusercontent.com/<you>/veil/main/deploy/install.sh \
#      | sudo bash -s -- --domain node.example.com
#
#  Uninstall:
#    curl -fsSL https://raw.githubusercontent.com/<you>/veil/main/deploy/install.sh \
#      | sudo bash -s -- --uninstall
#
#  What install does
#    1. detect distro / arch / init system, install runtime deps
#    2. get the `veil` binary: local file > GitHub release > build from source
#    3. create the `veil` system user, /etc/veil, /var/lib/veil
#    4. generate config (fresh UUIDs, passwords, sub token) + CA/leaf certificate
#    5. install & enable a systemd (or OpenRC) unit: boot start + crash restart
#    6. apply BBR / socket buffer tuning (best effort)
#    7. print every share link, the admin token and the subscription URL
#
#  Flags
#    --domain <fqdn>      public domain (used for SNI and links)
#    --email <addr>       contact address
#    --repo <owner/name>  release repository        (default: veil-proxy/veil)
#    --version <tag>      release tag or "latest"
#    --home <dir>         config root                (default: /etc/veil)
#    --state <dir>        runtime state              (default: /var/lib/veil)
#    --source             always build from source
#    --mirror cn|official mirror for rustup / git clone
#    --no-tune            skip sysctl tuning
#    --uninstall          remove veil from this machine
#    --keep-config        with --uninstall: keep /etc/veil
#    --yes                with --uninstall: do not ask
# =============================================================================
set -euo pipefail

VEIL_REPO="${VEIL_REPO:-veil-proxy/veil}"
VEIL_VERSION="${VEIL_VERSION:-latest}"
VEIL_DOMAIN="${VEIL_DOMAIN:-}"
VEIL_EMAIL="${VEIL_EMAIL:-}"
VEIL_HOME="${VEIL_HOME:-/etc/veil}"
VEIL_STATE="${VEIL_STATE:-/var/lib/veil}"
SERVICE="veil"
MIRROR="official"
DO_UNINSTALL=0
KEEP_CONFIG=0
ASSUME_YES=0
FROM_SOURCE=0
DO_TUNE=1

BOLD=$'\033[1m'; DIM=$'\033[2m'; RED=$'\033[31m'; GREEN=$'\033[32m'
YELLOW=$'\033[33m'; CYAN=$'\033[36m'; RESET=$'\033[0m'

log()  { printf '%s==>%s %s\n' "$CYAN" "$RESET" "$*"; }
ok()   { printf '  %sok%s  %s\n' "$GREEN" "$RESET" "$*"; }
warn() { printf '  %swarn%s %s\n' "$YELLOW" "$RESET" "$*"; }
die()  { printf '%sfail%s %s\n' "$RED" "$RESET" "$*" >&2; exit 1; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --domain)      VEIL_DOMAIN="${2:-}"; shift 2 ;;
    --email)       VEIL_EMAIL="${2:-}"; shift 2 ;;
    --repo)        VEIL_REPO="${2:-}"; shift 2 ;;
    --version)     VEIL_VERSION="${2:-}"; shift 2 ;;
    --home)        VEIL_HOME="${2:-}"; shift 2 ;;
    --state)       VEIL_STATE="${2:-}"; shift 2 ;;
    --mirror)      MIRROR="${2:-official}"; shift 2 ;;
    --source)      FROM_SOURCE=1; shift ;;
    --no-tune)     DO_TUNE=0; shift ;;
    --uninstall)   DO_UNINSTALL=1; shift ;;
    --keep-config) KEEP_CONFIG=1; shift ;;
    -y|--yes)      ASSUME_YES=1; shift ;;
    -h|--help)     sed -n '2,40p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown argument: $1 (try --help)" ;;
  esac
done

# ---------------------------------------------------------------------------
# environment
# ---------------------------------------------------------------------------
need_root() { [[ "$(id -u)" -eq 0 ]] || die "run as root: prefix with sudo"; }

detect_os() {
  [[ -f /etc/os-release ]] || die "unsupported system: no /etc/os-release"
  # shellcheck disable=SC1091
  . /etc/os-release
  OS_ID="${ID:-unknown}"
  OS_NAME="${PRETTY_NAME:-$OS_ID}"
}

detect_arch() {
  case "$(uname -m)" in
    x86_64|amd64)  ARCH=x86_64 ;;
    aarch64|arm64) ARCH=aarch64 ;;
    *) die "unsupported architecture: $(uname -m)" ;;
  esac
  TARGET="${ARCH}-unknown-linux-musl"
}

PM=""
detect_pm() {
  for c in apt-get dnf yum apk pacman zypper; do
    command -v "$c" >/dev/null 2>&1 && { PM="$c"; return; }
  done
  PM=""
}

pm_install() {
  [[ $# -gt 0 ]] || return 0
  case "$PM" in
    apt-get) DEBIAN_FRONTEND=noninteractive apt-get update -qq && apt-get install -y -qq "$@" ;;
    dnf)     dnf install -y -q "$@" ;;
    yum)     yum install -y -q "$@" ;;
    apk)     apk add --no-cache "$@" ;;
    pacman)  pacman -Sy --noconfirm --needed "$@" ;;
    zypper)  zypper --non-interactive install "$@" ;;
    *)       return 1 ;;
  esac
}

has_systemd() { command -v systemctl >/dev/null 2>&1 && [[ -d /run/systemd/system ]]; }
has_openrc()  { command -v rc-update >/dev/null 2>&1; }

require_tools() {
  local missing=()
  for t in "$@"; do command -v "$t" >/dev/null 2>&1 || missing+=("$t"); done
  [[ ${#missing[@]} -eq 0 ]] && return 0
  log "installing: ${missing[*]}"
  detect_pm
  [[ -n "$PM" ]] || die "no package manager found; install ${missing[*]} manually"
  local pkgs=()
  for t in "${missing[@]}"; do
    case "$t" in
      curl) pkgs+=("curl") ;;
      tar)  pkgs+=("tar") ;;
      *)    pkgs+=("$t") ;;
    esac
  done
  pm_install ca-certificates "${pkgs[@]}" || pm_install "${pkgs[@]}" \
    || die "failed to install ${missing[*]}"
}

# ---------------------------------------------------------------------------
# uninstall
# ---------------------------------------------------------------------------
do_uninstall() {
  need_root
  echo
  printf '%s卸载 veil%s\n' "$BOLD" "$RESET"
  echo

  if [[ $ASSUME_YES -eq 0 ]]; then
    printf '将删除：\n'
    printf '  · systemd/OpenRC 服务（停止并取消开机自启）\n'
    printf '  · /usr/local/bin/veil\n'
    [[ $KEEP_CONFIG -eq 1 ]] && printf '  · 保留配置目录 %s\n' "$VEIL_HOME" \
                             || printf '  · 配置与证书 %s（含所有账号凭据）\n' "$VEIL_HOME"
    printf '  · 运行状态 %s\n' "$VEIL_STATE"
    echo
    read -r -p "确认继续？输入 yes: " answer
    [[ "$answer" == "yes" ]] || { echo "已取消"; exit 0; }
  fi

  log "停止服务"
  if has_systemd; then
    systemctl disable --now "$SERVICE" >/dev/null 2>&1 || true
    systemctl stop "$SERVICE" >/dev/null 2>&1 || true
    rm -f "/etc/systemd/system/$SERVICE.service"
    systemctl daemon-reload || true
    ok "systemd unit removed"
  elif has_openrc; then
    rc-service "$SERVICE" stop >/dev/null 2>&1 || true
    rc-update del "$SERVICE" default >/dev/null 2>&1 || true
    rm -f "/etc/init.d/$SERVICE"
    ok "OpenRC script removed"
  else
    pkill -f '/usr/local/bin/veil run' >/dev/null 2>&1 || true
    warn "no init system detected; killed any running veil process"
  fi

  log "删除文件"
  rm -f /usr/local/bin/veil
  rm -rf "$VEIL_STATE"
  if [[ $KEEP_CONFIG -eq 1 ]]; then
    warn "配置保留在 $VEIL_HOME"
  else
    rm -rf "$VEIL_HOME"
  fi
  rm -f /etc/sysctl.d/99-veil.conf
  ok "binary, state and tuning removed"

  if command -v getent >/dev/null 2>&1 && getent passwd veil >/dev/null 2>&1; then
    if command -v userdel >/dev/null 2>&1; then
      userdel veil >/dev/null 2>&1 || warn "could not delete user 'veil' (files may still be owned by it)"
    fi
  fi

  echo
  printf '%sveil 已卸载%s\n' "$GREEN" "$RESET"
  [[ $KEEP_CONFIG -eq 1 ]] && echo "配置仍在 $VEIL_HOME，如需彻底清除：rm -rf $VEIL_HOME"
  echo
  exit 0
}

# ---------------------------------------------------------------------------
# binary acquisition
# ---------------------------------------------------------------------------
get_binary() {
  local dest="$1"
  if [[ -n "${VEIL_BIN:-}" ]]; then
    log "using local binary: $VEIL_BIN"
    [[ -f "$VEIL_BIN" ]] || die "$VEIL_BIN does not exist"
    cp "$VEIL_BIN" "$dest"
    chmod +x "$dest"
    return 0
  fi

  if [[ $FROM_SOURCE -eq 0 ]]; then
    # Prefer a fully static build; fall back to the glibc one for arm64 because
    # aarch64-musl needs a cross toolchain the release job does not have.
    local assets=()
    case "$ARCH" in
      x86_64)  assets=("x86_64-unknown-linux-musl") ;;
      aarch64) assets=("aarch64-unknown-linux-musl" "aarch64-unknown-linux-gnu") ;;
    esac

    local t url
    for t in "${assets[@]}"; do
      if [[ "$VEIL_VERSION" == "latest" ]]; then
        url="https://github.com/${VEIL_REPO}/releases/latest/download/veil-${t}.tar.gz"
      else
        url="https://github.com/${VEIL_REPO}/releases/download/${VEIL_VERSION}/veil-${t}.tar.gz"
      fi
      log "downloading ${t}"
      if curl -fsSL --retry 2 --connect-timeout 15 -o "$dest.tar.gz" "$url"; then
        mkdir -p "$dest.d"
        if tar -xzf "$dest.tar.gz" -C "$dest.d" 2>/dev/null && [[ -f "$dest.d/veil" ]]; then
          mv "$dest.d/veil" "$dest"
          chmod +x "$dest"
          rm -rf "$dest.d" "$dest.tar.gz"
          ok "downloaded $(basename "$url")"
          return 0
        fi
        warn "archive for ${t} was not usable"
        rm -rf "$dest.d" "$dest.tar.gz"
      fi
    done
    warn "no release asset for ${ARCH} — falling back to a source build"
  fi

  log "building from source (Rust + gcc, a few minutes)"
  build_from_source "$dest"
}

build_from_source() {
  local dest="$1"
  [[ -n "$PM" ]] || detect_pm
  case "$PM" in
    apt-get) pm_install build-essential pkg-config || true ;;
    apk)     pm_install build-base || true ;;
    dnf|yum) pm_install gcc gcc-c++ make || true ;;
    pacman)  pm_install base-devel || true ;;
    zypper)  pm_install gcc make || true ;;
  esac
  command -v gcc >/dev/null 2>&1 || die "a C compiler is required to build (install gcc / build-essential)"
  command -v curl >/dev/null 2>&1 || die "curl is required"

  local rustup_url="https://sh.rustup.rs"
  local clone_url="https://github.com/${VEIL_REPO}.git"
  if [[ "$MIRROR" == "cn" ]]; then
    export RUSTUP_DIST_SERVER="https://rsproxy.cn"
    export RUSTUP_UPDATE_ROOT="https://rsproxy.cn/rustup"
    clone_url="https://ghfast.top/https://github.com/${VEIL_REPO}.git"
    ok "using CN mirrors (rsproxy / ghfast)"
  fi

  if ! command -v cargo >/dev/null 2>&1; then
    log "installing Rust toolchain"
    curl -fsSL "$rustup_url" | sh -s -- -y --profile minimal >/dev/null 2>&1 \
      || die "rustup install failed"
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
  fi
  command -v cargo >/dev/null 2>&1 || die "cargo not on PATH after rustup"

  local src
  src="$(mktemp -d)"
  log "fetching source from ${VEIL_REPO}"
  git clone --depth 1 "$clone_url" "$src/veil" >/dev/null 2>&1 \
    || die "git clone failed (private repo? pass VEIL_BIN=/path/to/veil instead)"

  log "compiling (this is the slow part)"
  ( cd "$src/veil" && cargo build --release ) || die "cargo build failed"
  cp "$src/veil/target/release/veil" "$dest"
  chmod +x "$dest"
  rm -rf "$src"
  ok "compiled $(basename "$dest")"
}

# ---------------------------------------------------------------------------
# helpers
# ---------------------------------------------------------------------------
port_free() {
  local port="$1"
  if command -v ss >/dev/null 2>&1; then
    ! ss -Hltn "sport = :$port" 2>/dev/null | grep -q .
  else
    ! (exec 3<>"/dev/tcp/127.0.0.1/$port") >/dev/null 2>&1
  fi
}

systemd_unit() {
  cat <<UNIT
[Unit]
Description=Veil proxy server
Documentation=https://github.com/${VEIL_REPO}
After=network-online.target nss-lookup.target
Wants=network-online.target
# Never stop retrying after a crash.
StartLimitIntervalSec=0

[Service]
Type=simple
User=veil
Group=veil
ExecStart=/usr/local/bin/veil run --config ${VEIL_HOME}/veil.toml
Restart=always
RestartSec=3
# Give up only if it dies 5 times within 10s *after* running fine for a while.
TimeoutStopSec=10
LimitNOFILE=1048576
LimitNPROC=65535

NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
PrivateDevices=true
ProtectKernelTunables=true
ProtectControlGroups=true
RestrictSUIDSGID=true
RestrictNamespaces=true
LockPersonality=true
MemoryDenyWriteExecute=true
SystemCallArchitectures=native
ReadWritePaths=${VEIL_HOME} ${VEIL_STATE}
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE

StandardOutput=journal
StandardError=journal
SyslogIdentifier=veil

[Install]
WantedBy=multi-user.target
UNIT
}

openrc_script() {
  cat <<UNIT
#!/sbin/openrc-run
name="veil"
description="Veil proxy server"
command="/usr/local/bin/veil"
command_args="run --config ${VEIL_HOME}/veil.toml"
command_user="veil:veil"
command_background=true
pidfile="/run/\${RC_SVCNAME}.pid"
output_log="/var/log/veil.log"
error_log="/var/log/veil.log"
supervisor="supervise-daemon"
respawn_delay=3
respawn_max=0

depend() {
    need net
    after firewall
}
UNIT
}

install_service() {
  if has_systemd; then
    systemd_unit >"/etc/systemd/system/${SERVICE}.service"
    systemctl daemon-reload
    systemctl enable "$SERVICE" >/dev/null 2>&1 || true
    systemctl restart "$SERVICE" || true
    return 0
  fi
  if has_openrc; then
    openrc_script >"/etc/init.d/$SERVICE"
    chmod +x "/etc/init.d/$SERVICE"
    rc-update add "$SERVICE" default >/dev/null 2>&1 || true
    rc-service "$SERVICE" restart >/dev/null 2>&1 || rc-service "$SERVICE" start || true
    return 0
  fi
  warn "no systemd / OpenRC found — veil is installed but not supervised"
  warn "start it manually: /usr/local/bin/veil run --config ${VEIL_HOME}/veil.toml"
  return 1
}

service_state() {
  if has_systemd; then
    systemctl is-active --quiet "$SERVICE" && echo active || echo inactive
  elif has_openrc; then
    rc-service "$SERVICE" status >/dev/null 2>&1 && echo active || echo inactive
  else
    pgrep -f '/usr/local/bin/veil run' >/dev/null 2>&1 && echo active || echo inactive
  fi
}

# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------
[[ $DO_UNINSTALL -eq 1 ]] && do_uninstall

need_root
detect_os
detect_arch
detect_pm

printf '\n%s╭──────────────────────────────────────────────╮%s\n' "$BOLD" "$RESET"
printf '%s│  veil installer                              │%s\n' "$BOLD" "$RESET"
printf '%s╰──────────────────────────────────────────────╯%s\n' "$BOLD" "$RESET"
printf '  system   %s (%s)\n' "$OS_NAME" "$ARCH"
printf '  repo     %s @ %s\n' "$VEIL_REPO" "$VEIL_VERSION"
printf '  config   %s\n\n' "$VEIL_HOME"

log "checking dependencies"
require_tools curl tar
ok "curl, tar present"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

get_binary "$TMP/veil"
install -m 0755 "$TMP/veil" /usr/local/bin/veil
VEIL_VER="$(/usr/local/bin/veil version 2>/dev/null | head -1 || echo veil)"
ok "/usr/local/bin/veil  ($VEIL_VER)"

log "preparing user and directories"
if ! id veil >/dev/null 2>&1; then
  useradd --system --home "$VEIL_STATE" --shell /usr/sbin/nologin veil 2>/dev/null \
    || adduser -S -h "$VEIL_STATE" -s /sbin/nologin veil 2>/dev/null \
    || warn "could not create the 'veil' user; running as root"
  ok "created system user 'veil'"
else
  ok "system user 'veil' already exists"
fi
install -d -m 0755 "$VEIL_HOME" "$VEIL_HOME/cert"
install -d -m 0750 -o veil -g veil "$VEIL_STATE" 2>/dev/null || install -d -m 0750 "$VEIL_STATE"

CONFIG="$VEIL_HOME/veil.toml"
if [[ -f "$CONFIG" ]]; then
  warn "$CONFIG exists — keeping it (delete it and re-run for a fresh setup)"
else
  log "generating configuration"
  /usr/local/bin/veil init -c "$CONFIG" --domain "$VEIL_DOMAIN" --email "$VEIL_EMAIL" >/dev/null
  chown -R veil:veil "$VEIL_HOME" 2>/dev/null || true
  chmod 0600 "$CONFIG"
  ok "wrote $CONFIG"
fi

# 443 is very often taken (nginx / caddy / another panel): move that one inbound
# instead of letting the service crash-loop on bind().
if ! port_free 443 && grep -q 'listen = "0.0.0.0:443"' "$CONFIG"; then
    for alt in 8443 9443 2053 2083; do
      if port_free "$alt" && ! grep -q "0.0.0.0:$alt" "$CONFIG"; then
        sed -i "s|listen = \"0.0.0.0:443\"|listen = \"0.0.0.0:$alt\"|" "$CONFIG"
        warn "port 443 is busy — moved the vless-tls inbound to $alt"
        break
      fi
    done
fi

log "installing service"
if install_service; then
  ok "boot start + crash restart enabled"
fi
sleep 1
STATE="$(service_state)"
if [[ "$STATE" == "active" ]]; then
  ok "veil is running"
else
  warn "service is not active yet — check: journalctl -u veil -n 50 --no-pager"
fi

if [[ $DO_TUNE -eq 1 && -w /etc/sysctl.d ]]; then
  log "applying network tuning"
  cat >/etc/sysctl.d/99-veil.conf <<'SYSCTL'
net.core.default_qdisc = fq
net.ipv4.tcp_congestion_control = bbr
net.core.rmem_max = 33554432
net.core.wmem_max = 33554432
net.ipv4.tcp_fastopen = 3
net.ipv4.tcp_mtu_probing = 1
SYSCTL
  sysctl --system >/dev/null 2>&1 || true
  ok "BBR + socket buffers"
fi

# ---------------------------------------------------------------------------
# summary
# ---------------------------------------------------------------------------
ADMIN_TOKEN="$(grep -m1 '^admin_token' "$CONFIG" | cut -d'"' -f2 || true)"
SUB_TOKEN="$(grep -m1 '^sub_token' "$CONFIG" | cut -d'"' -f2 || true)"
API_LISTEN="$(grep -A6 '^\[api\]' "$CONFIG" | grep -m1 '^listen' | cut -d'"' -f2 || true)"
API_PORT="${API_LISTEN##*:}"
USER_NAME="$(/usr/local/bin/veil user list -c "$CONFIG" 2>/dev/null | awk 'NR==2{print $1}' || true)"

echo
printf '%s╭──────────────────────────────────────────────────────────────╮%s\n' "$GREEN" "$RESET"
printf '%s│  安装完成                                                    │%s\n' "$GREEN" "$RESET"
printf '%s╰──────────────────────────────────────────────────────────────╯%s\n' "$GREEN" "$RESET"
echo
printf '%s配置%s      %s\n' "$BOLD" "$RESET" "$CONFIG"
printf '%s服务%s      systemctl status veil   （开机自启 + 崩溃自动重启）\n' "$BOLD" "$RESET"
printf '%s日志%s      journalctl -u veil -f\n' "$BOLD" "$RESET"
printf '%s管理令牌%s  %s\n' "$BOLD" "$RESET" "$ADMIN_TOKEN"
echo
printf '%s节点链接%s（用户 %s）\n' "$BOLD" "$RESET" "${USER_NAME:-default}"
/usr/local/bin/veil link -c "$CONFIG" 2>/dev/null | sed 's/^/  /'
echo
if [[ -n "$SUB_TOKEN" ]]; then
  printf '%s订阅地址%s\n' "$BOLD" "$RESET"
  printf '  本机   http://127.0.0.1:%s/sub/%s\n' "${API_PORT:-8080}" "$SUB_TOKEN"
  [[ -n "$VEIL_DOMAIN" ]] && printf '  公网   https://%s/sub/%s   %s(需先用 nginx 反代 127.0.0.1:%s)%s\n' \
    "$VEIL_DOMAIN" "$SUB_TOKEN" "$DIM" "${API_PORT:-8080}" "$RESET"
  echo
fi
printf '%s还需要做的事%s\n' "$BOLD" "$RESET"
echo "  1. 域名 A 记录指向本机：${VEIL_DOMAIN:-<你的域名>}"
echo "  2. 防火墙放行：443(或安装时自动改的端口)/tcp、8443-8445/tcp、8446/udp"
echo "  3. 换真证书：veil cert show → 见 docs/DEPLOY.md"
echo
printf '%s卸载%s\n' "$BOLD" "$RESET"
echo "  curl -fsSL https://raw.githubusercontent.com/${VEIL_REPO}/main/deploy/install.sh | sudo bash -s -- --uninstall"
echo
