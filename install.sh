#!/usr/bin/env bash

set -euo pipefail

REPO="https://raw.githubusercontent.com/geekerlw/methodus/main"

cat <<'BANNER'
  __  __      _   _               _
 |  \/  | ___| |_| |__   ___   __| |_   _ ___
 | |\/| |/ _ \ __| '_ \ / _ \ / _` | | | / __|
 | |  | |  __/ |_| | | | (_) | (_| | |_| \__ \
 |_|  |_|\___|\__|_| |_|\___/ \__,_|\__,_|___/

BANNER
echo "  Dynamic Skill Planning Agent"
echo "  Author : Steven Lee"
echo ""

# ── platform selection ────────────────────────────────────────────────────────

PLATFORMS=("claude" "cursor")
SELECTED_PLATFORMS=()

if [[ -n "${METHODUS_PLATFORMS:-}" ]]; then
  IFS=',' read -ra SELECTED_PLATFORMS <<< "$METHODUS_PLATFORMS"
elif [[ -t 1 ]]; then
  printf "Which platforms to install? (options: claude, cursor, all) [all]: " > /dev/tty
  if read -r platform_input < /dev/tty; then
    :
  else
    platform_input="all"
  fi
  platform_input="${platform_input:-all}"
  if [[ "$platform_input" == "all" ]]; then
    SELECTED_PLATFORMS=("${PLATFORMS[@]}")
  else
    IFS=',' read -ra SELECTED_PLATFORMS <<< "$platform_input"
  fi
else
  echo "Non-interactive session detected → installing to all platforms."
  echo "Set METHODUS_PLATFORMS=claude or METHODUS_PLATFORMS=cursor to limit."
  SELECTED_PLATFORMS=("${PLATFORMS[@]}")
fi

# ── scope selection ───────────────────────────────────────────────────────────

scope=""

if [[ -n "${METHODUS_SCOPE:-}" ]]; then
  scope="$METHODUS_SCOPE"
elif [[ -t 1 ]]; then
  printf "Install scope — global or project? [global]: " > /dev/tty
  if read -r scope_input < /dev/tty; then
    :
  else
    scope_input="global"
  fi
  scope="${scope_input:-global}"
else
  echo "Non-interactive session detected → installing globally."
  echo "Set METHODUS_SCOPE=project to install into the current directory."
  scope="global"
fi

# ── download ──────────────────────────────────────────────────────────────────

TMP=$(mktemp)
trap 'rm -f "$TMP"' EXIT

echo ""
echo "⬇️  Downloading methodus agent..."
if command -v curl >/dev/null 2>&1; then
  curl -fsSL "$REPO/methodus.md" -o "$TMP"
elif command -v wget >/dev/null 2>&1; then
  wget -q "$REPO/methodus.md" -O "$TMP"
else
  echo "❌ curl or wget is required."
  exit 1
fi

# ── install ───────────────────────────────────────────────────────────────────

install_to() {
  local base="$1"
  local dir="$base/agents"
  mkdir -p "$dir"
  cp "$TMP" "$dir/methodus.md"
  echo "  ✓ $dir/methodus.md"
}

installed=0

for platform in "${SELECTED_PLATFORMS[@]}"; do
  platform="$(echo "$platform" | xargs)"  # trim whitespace
  if [[ "$scope" == "project" ]]; then
    base="$(pwd)/.$platform"
  else
    base="$HOME/.$platform"
  fi
  install_to "$base"
  installed=$((installed + 1))
done

mkdir -p "$HOME/.methodus"

cat <<EOF

✅ Installation Complete
────────────────────────
  Platforms : ${SELECTED_PLATFORMS[*]}
  Scope     : $scope
  Experience: ~/.methodus/

Invoke in Claude Code or Cursor:
  @methodus <your goal>
EOF
