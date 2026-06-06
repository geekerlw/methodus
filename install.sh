#!/usr/bin/env bash

set -euo pipefail

REPO="https://raw.githubusercontent.com/geekerlw/methodus/main"
TTY=/dev/tty

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

# ── interactive UI ────────────────────────────────────────────────────────────

# multi_select <prompt> <opt1> <opt2> ...  →  MULTI_RESULT array
multi_select() {
  local prompt="$1"; shift
  local options=("$@")
  local n=${#options[@]}
  local cursor=0
  local i
  local selected=()
  for ((i=0; i<n; i++)); do selected[$i]=1; done  # default: all selected

  _ms_draw() {
    for ((i=0; i<n; i++)); do
      local box="[ ]"
      [[ "${selected[$i]}" -eq 1 ]] && box="[x]"
      if [[ $i -eq $cursor ]]; then
        printf "\r\033[K  \033[36m❯\033[0m \033[1m%s %s\033[0m\n" "$box" "${options[$i]}" > "$TTY"
      else
        printf "\r\033[K    %s %s\n" "$box" "${options[$i]}" > "$TTY"
      fi
    done
  }

  printf "\n%s\n" "$prompt" > "$TTY"
  printf "  \033[2m↑↓ navigate · space toggle · enter confirm\033[0m\n" > "$TTY"
  tput civis > "$TTY" 2>/dev/null || true
  _ms_draw

  while true; do
    printf "\033[%dA" "$n" > "$TTY"
    local key=""
    IFS= read -rsn1 key < "$TTY" || true
    if [[ "$key" == $'\x1b' ]]; then
      local k2="" k3=""
      IFS= read -rsn1 k2 < "$TTY" || true
      IFS= read -rsn1 k3 < "$TTY" || true
      case "$k2$k3" in
        '[A') [[ $cursor -gt 0 ]] && cursor=$((cursor - 1)) ;;
        '[B') [[ $cursor -lt $((n-1)) ]] && cursor=$((cursor + 1)) ;;
      esac
    elif [[ "$key" == " " ]]; then
      selected[$cursor]=$(( 1 - selected[$cursor] ))
    elif [[ -z "$key" ]]; then
      break
    fi
    _ms_draw
  done

  tput cnorm > "$TTY" 2>/dev/null || true
  printf "\n" > "$TTY"

  MULTI_RESULT=()
  for ((i=0; i<n; i++)); do
    [[ "${selected[$i]}" -eq 1 ]] && MULTI_RESULT+=("${options[$i]}")
  done
}

# single_select <prompt> <opt1> <opt2> ...  →  SINGLE_RESULT
single_select() {
  local prompt="$1"; shift
  local options=("$@")
  local n=${#options[@]}
  local cursor=0
  local i

  _ss_draw() {
    for ((i=0; i<n; i++)); do
      if [[ $i -eq $cursor ]]; then
        printf "\r\033[K  \033[36m❯\033[0m \033[1m(*) %s\033[0m\n" "${options[$i]}" > "$TTY"
      else
        printf "\r\033[K    ( ) %s\n" "${options[$i]}" > "$TTY"
      fi
    done
  }

  printf "\n%s\n" "$prompt" > "$TTY"
  printf "  \033[2m↑↓ navigate · enter confirm\033[0m\n" > "$TTY"
  tput civis > "$TTY" 2>/dev/null || true
  _ss_draw

  while true; do
    printf "\033[%dA" "$n" > "$TTY"
    local key=""
    IFS= read -rsn1 key < "$TTY" || true
    if [[ "$key" == $'\x1b' ]]; then
      local k2="" k3=""
      IFS= read -rsn1 k2 < "$TTY" || true
      IFS= read -rsn1 k3 < "$TTY" || true
      case "$k2$k3" in
        '[A') [[ $cursor -gt 0 ]] && cursor=$((cursor - 1)) ;;
        '[B') [[ $cursor -lt $((n-1)) ]] && cursor=$((cursor + 1)) ;;
      esac
    elif [[ -z "$key" ]]; then
      break
    fi
    _ss_draw
  done

  tput cnorm > "$TTY" 2>/dev/null || true
  printf "\n" > "$TTY"

  SINGLE_RESULT="${options[$cursor]}"
}

# ── platform selection ────────────────────────────────────────────────────────

SELECTED_PLATFORMS=()

if [[ -n "${METHODUS_PLATFORMS:-}" ]]; then
  IFS=',' read -ra SELECTED_PLATFORMS <<< "$METHODUS_PLATFORMS"
elif [[ -t 1 ]]; then
  multi_select "Which platforms to install?" "claude" "cursor"
  SELECTED_PLATFORMS=("${MULTI_RESULT[@]}")
  [[ ${#SELECTED_PLATFORMS[@]} -eq 0 ]] && SELECTED_PLATFORMS=("claude" "cursor")
else
  echo "Non-interactive session → installing to all platforms."
  echo "Set METHODUS_PLATFORMS=claude or METHODUS_PLATFORMS=cursor to limit."
  SELECTED_PLATFORMS=("claude" "cursor")
fi

# ── scope selection ───────────────────────────────────────────────────────────

scope=""

if [[ -n "${METHODUS_SCOPE:-}" ]]; then
  scope="$METHODUS_SCOPE"
elif [[ -t 1 ]]; then
  single_select "Install scope?" "global (~/.{platform}/agents/)" "project (./{platform}/agents/)"
  [[ "$SINGLE_RESULT" == global* ]] && scope="global" || scope="project"
else
  echo "Non-interactive session → installing globally."
  echo "Set METHODUS_SCOPE=project to install into the current directory."
  scope="global"
fi

# ── download ──────────────────────────────────────────────────────────────────

TMP=$(mktemp)
trap 'rm -f "$TMP"' EXIT

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
  platform="$(printf '%s' "$platform" | xargs)"
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
