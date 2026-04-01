#!/usr/bin/env bash
set -euo pipefail

# ─── Token Scrooge installer ──────────────────────────────────────────────────
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Eimisas/scrooge/master/install.sh | bash
#   OR: bash install.sh   (from a local clone)

BINARY_NAME="scrooge"
INSTALL_DIR="${HOME}/.local/bin"
REPO="Eimisas/token-scrooge"

RED=$'\033[0;31m'; GREEN=$'\033[0;32m'; YELLOW=$'\033[1;33m'; BOLD=$'\033[1m'; NC=$'\033[0m'
info()    { echo -e "${BOLD}${GREEN}[scrooge]${NC} $*"; }
warn()    { echo -e "${YELLOW}[scrooge]${NC} $*"; }
die()     { echo -e "${RED}[scrooge] error:${NC} $*" >&2; exit 1; }

# ─── Detect platform ──────────────────────────────────────────────────────────
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Darwin) OS_SLUG="macos" ;;
  Linux)  OS_SLUG="linux" ;;
  *)      die "Unsupported OS: $OS" ;;
esac

case "$ARCH" in
  x86_64)        ARCH_SLUG="x86_64"  ;;
  arm64|aarch64) ARCH_SLUG="aarch64" ;;
  *)             die "Unsupported architecture: $ARCH" ;;
esac

PLATFORM="${OS_SLUG}-${ARCH_SLUG}"

# ─── Install from source if Cargo is available and we're in the repo ──────────
# Detect whether we're at the repo root (install.sh location) or inside scrooge/
if [[ -f "scrooge/Cargo.toml" ]] && grep -q 'name = "scrooge"' scrooge/Cargo.toml 2>/dev/null; then
  SOURCE_DIR="scrooge"
elif [[ -f "Cargo.toml" ]] && grep -q 'name = "scrooge"' Cargo.toml 2>/dev/null; then
  SOURCE_DIR="."
else
  SOURCE_DIR=""
fi

if [[ -n "$SOURCE_DIR" ]]; then
  info "Building from source…"
  if ! command -v cargo &>/dev/null; then
    die "cargo not found. Install Rust from https://rustup.rs or download a pre-built binary."
  fi
  cargo build --release --manifest-path "${SOURCE_DIR}/Cargo.toml"
  BINARY="${SOURCE_DIR}/target/release/${BINARY_NAME}"

# ─── Download pre-built binary from GitHub Releases ──────────────────────────
elif command -v curl &>/dev/null; then
  info "Fetching latest release for ${PLATFORM}…"

  LATEST=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | grep '"tag_name"' | head -1 | cut -d'"' -f4)
  [[ -z "$LATEST" ]] && die "Could not determine latest release. Check ${REPO} exists."

  URL="https://github.com/${REPO}/releases/download/${LATEST}/${BINARY_NAME}-${PLATFORM}"
  TMPFILE="$(mktemp)"
  trap "rm -f $TMPFILE" EXIT

  info "Downloading ${BINARY_NAME} ${LATEST} for ${PLATFORM}…"
  curl -fsSL --progress-bar "$URL" -o "$TMPFILE" \
    || die "Download failed. Check https://github.com/${REPO}/releases for available assets."

  chmod +x "$TMPFILE"
  BINARY="$TMPFILE"

else
  die "Neither cargo nor curl found. Install one and try again."
fi

# ─── Install binary ───────────────────────────────────────────────────────────
mkdir -p "$INSTALL_DIR"
cp "$BINARY" "${INSTALL_DIR}/${BINARY_NAME}"
chmod +x "${INSTALL_DIR}/${BINARY_NAME}"
info "Installed → ${INSTALL_DIR}/${BINARY_NAME}"

# ─── Ensure install dir is on PATH ───────────────────────────────────────────
if ! echo "$PATH" | tr ':' '\n' | grep -qx "$INSTALL_DIR"; then
  SHELL_RC=""
  if [[ -n "${ZSH_VERSION:-}" ]] || [[ "$SHELL" == */zsh ]]; then
    SHELL_RC="${HOME}/.zshrc"
  elif [[ -n "${BASH_VERSION:-}" ]] || [[ "$SHELL" == */bash ]]; then
    SHELL_RC="${HOME}/.bashrc"
    [[ "$(uname)" == "Darwin" ]] && SHELL_RC="${HOME}/.bash_profile"
  fi

  if [[ -n "$SHELL_RC" ]]; then
    echo ""                                      >> "$SHELL_RC"
    echo "# Token Scrooge"                       >> "$SHELL_RC"
    echo "export PATH=\"\$PATH:${INSTALL_DIR}\"" >> "$SHELL_RC"
    warn "${INSTALL_DIR} added to PATH in ${SHELL_RC}"
    warn "Run: source ${SHELL_RC}"
  else
    warn "Add ${INSTALL_DIR} to your PATH manually."
  fi
fi

# ─── First-run setup ─────────────────────────────────────────────────────────
info "Running first-time setup…"
"${INSTALL_DIR}/${BINARY_NAME}" setup

echo ""
echo -e "${BOLD}Done.${NC}"
echo ""
echo "  Use ${BOLD}scrooge claude${NC} instead of ${BOLD}claude${NC}."
echo ""
echo "  ${BOLD}Next steps:${NC}"
echo "    1. Start the claude with scrooge memory:"
echo "       ${BOLD}scrooge claude${NC}"
echo ""
echo "  Commands:"
echo "    scrooge daemon start|status|stop   manage memory assistant manually"
echo "    scrooge remember \"<fact>\"    save a fact manually"
echo "    scrooge recall \"<query>\"     search memory"
echo "    scrooge --savings            token savings report"
echo ""
