#!/usr/bin/env bash
# =============================================================================
#  Publish this repository to GitHub and cut the first release tag.
#
#    bash publish.sh              # repo name: veil
#    bash publish.sh my-node      # custom name
#
#  Requirements: git, and either
#    * GitHub CLI logged in  -> gh auth login
#    * or an HTTPS remote you have push rights to (Git Credential Manager will
#      open a browser window on first push)
#
#  Why a script: the four commands are easy to get subtly wrong (missing
#  --source=., forgetting the tag, tagging before the remote exists).
# =============================================================================
set -euo pipefail

NAME="${1:-veil}"
TAG="${TAG:-v0.1.0}"
HERE="$(cd "$(dirname "$0")" && pwd)"
cd "$HERE"

GREEN=$'\033[32m'; YELLOW=$'\033[33m'; RED=$'\033[31m'; DIM=$'\033[2m'; RESET=$'\033[0m'
ok()   { printf '  %sok%s %s\n' "$GREEN" "$RESET" "$*"; }
warn() { printf '  %swarn%s %s\n' "$YELLOW" "$RESET" "$*"; }
die()  { printf '%sfail%s %s\n' "$RED" "$RESET" "$*" >&2; exit 1; }

command -v git >/dev/null 2>&1 || die "git not found"
[[ -d .git ]] || die "not a git repository — run this from the veil folder"

if [[ -n "$(git status --porcelain)" ]]; then
  warn "there are uncommitted changes; committing them first"
  git add -A
  git commit -m "wip: publish snapshot" >/dev/null
fi

BRANCH="$(git rev-parse --abbrev-ref HEAD)"
[[ "$BRANCH" == "main" ]] || { git branch -M main && BRANCH=main; }

LOGIN=""
if command -v gh >/dev/null 2>&1 && gh auth status >/dev/null 2>&1; then
  LOGIN="$(gh api user --jq .login 2>/dev/null || true)"
fi

if [[ -n "$LOGIN" ]]; then
  ok "gh is authenticated as $LOGIN"
  if git remote get-url origin >/dev/null 2>&1; then
    ok "origin already configured: $(git remote get-url origin)"
  else
    printf '  creating https://github.com/%s/%s\n' "$LOGIN" "$NAME"
    gh repo create "$NAME" --public --source=. --remote=origin --push \
      || die "gh repo create failed (does $LOGIN/$NAME already exist?)"
    ok "repository created and pushed"
  fi
  git push -u origin main
else
  warn "gh is not logged in"
  if git remote get-url origin >/dev/null 2>&1; then
    ok "origin already configured: $(git remote get-url origin)"
    echo
    echo "  pushing now — Git Credential Manager may open a browser window:"
    git push -u origin main
  else
    cat <<TXT

  两种方式任选一种，然后重新运行本脚本：

  A) 用 GitHub CLI（推荐）
       gh auth login
       bash publish.sh

  B) 先在网页上建一个空仓库（不要勾 README / .gitignore / license）
     https://github.com/new?name=$NAME
     然后：
       git remote add origin https://github.com/<你的用户名>/$NAME.git
       bash publish.sh

TXT
    exit 1
  fi
fi

# --- tag: this is what triggers .github/workflows/release.yml ---------------
if git rev-parse "$TAG" >/dev/null 2>&1; then
  warn "tag $TAG already exists locally"
else
  git tag -a "$TAG" -m "veil $TAG"
  ok "created tag $TAG"
fi
git push origin "$TAG" 2>/dev/null || git push origin --tags
ok "tag pushed — GitHub Actions is now building the Linux binaries"

REPO_URL="$(git remote get-url origin | sed -e 's|^git@github.com:|https://github.com/|' -e 's|\.git$||')"
SLUG="${REPO_URL#https://github.com/}"

cat <<TXT

$(printf '%s' "$GREEN")已发布$(printf '%s' "$RESET")

  仓库       $REPO_URL
  Release    $REPO_URL/actions      (等 Actions 跑完，约 3-5 分钟)

$(printf '%s' "$DIM")装到服务器：$(printf '%s' "$RESET")

  REPO=$SLUG
  curl -fsSL https://raw.githubusercontent.com/\$REPO/main/deploy/install.sh \\
    | sudo bash -s -- --repo \$REPO --domain node.example.com --email me@example.com

$(printf '%s' "$DIM")卸载：$(printf '%s' "$RESET")

  curl -fsSL https://raw.githubusercontent.com/$SLUG/main/deploy/install.sh \\
    | sudo bash -s -- --uninstall

TXT
