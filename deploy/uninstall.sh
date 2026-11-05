#!/usr/bin/env bash
# Thin wrapper so `curl .../deploy/uninstall.sh | sudo bash` also works.
# The real logic lives in install.sh --uninstall (single source of truth).
set -euo pipefail

if [[ -n "${BASH_SOURCE[0]:-}" && -f "${BASH_SOURCE[0]}" ]]; then
  HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  exec bash "$HERE/install.sh" --uninstall "$@"
fi

# Piped: fetch the installer and hand over.
VER="${VEIL_VERSION:-main}"
REPO="${VEIL_REPO:-veil-proxy/veil}"
exec bash -c "curl -fsSL https://raw.githubusercontent.com/${REPO}/${VER}/deploy/install.sh | bash -s -- --uninstall $*"
