#!/usr/bin/env bash
set -e

REPO="https://raw.githubusercontent.com/geekerlw/methodus/main"

# ── helpers ──────────────────────────────────────────────────────────────────

prompt_choice() {
  local question="$1"; shift
  local options=("$@")
  echo ""
  echo "$question"
  for i in "${!options[@]}"; do
    echo "  $((i+1))) ${options[$i]}"
  done
  while true; do
    printf "Enter numbers separated by spaces (e.g. 1 2): "
    read -r input
    # validate: all tokens are valid indices
    local valid=true
    for tok in $input; do
      if ! [[ "$tok" =~ ^[0-9]+$ ]] || (( tok < 1 || tok > ${#options[@]} )); then
        echo "  Invalid choice: $tok. Please try again."
        valid=false
        break
      fi
    done
    $valid && break
  done
  CHOICE_RESULT="$input"
}

prompt_yn() {
  local question="$1"
  while true; do
    printf "%s [y/n]: " "$question"
    read -r yn
    case "$yn" in
      [Yy]*) return 0 ;;
      [Nn]*) return 1 ;;
      *) echo "  Please answer y or n." ;;
    esac
  done
}

install_to() {
  local base="$1"
  local dir="$base/agents"
  mkdir -p "$dir"
  cp "$TMP" "$dir/methodus.md"
  echo "  ✓ $dir/methodus.md"
}

# ── platform selection ────────────────────────────────────────────────────────

PLATFORMS=("Claude Code" "Cursor")
PLATFORM_DIRS=("claude" "cursor")

prompt_choice "Which platforms do you want to install methodus to?" "${PLATFORMS[@]}"
SELECTED_PLATFORMS="$CHOICE_RESULT"

# ── scope selection ───────────────────────────────────────────────────────────

echo ""
echo "Install scope:"
echo "  1) Global  — ~/.{platform}/agents/  (available in all projects)"
echo "  2) Project — .{platform}/agents/    (current directory only)"
while true; do
  printf "Choose scope [1/2]: "
  read -r scope
  case "$scope" in
    1|2) break ;;
    *) echo "  Please enter 1 or 2." ;;
  esac
done

# ── download ──────────────────────────────────────────────────────────────────

TMP=$(mktemp)
echo ""
echo "Downloading methodus agent..."
curl -fsSL "$REPO/methodus.md" -o "$TMP"

# ── install ───────────────────────────────────────────────────────────────────

installed=0

for idx in $SELECTED_PLATFORMS; do
  platform="${PLATFORM_DIRS[$((idx-1))]}"
  if [ "$scope" -eq 1 ]; then
    base="$HOME/.$platform"
  else
    base="$(pwd)/.$platform"
  fi
  install_to "$base"
  installed=$((installed + 1))
done

mkdir -p "$HOME/.methodus"
rm "$TMP"

echo ""
echo "methodus installed to $installed platform(s)."
echo "Experience store ready at ~/.methodus/"
