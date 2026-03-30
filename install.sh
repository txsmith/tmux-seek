#!/usr/bin/env bash
set -euo pipefail

REPO="txsmith/tmux-seek"
INSTALL_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Detect platform
OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
ARCH="$(uname -m)"

case "$OS" in
    linux)  OS="linux" ;;
    darwin) OS="darwin" ;;
    *) echo "seek: unsupported OS: $OS" >&2; exit 1 ;;
esac

case "$ARCH" in
    x86_64|amd64)  ARCH="x86_64" ;;
    aarch64|arm64) ARCH="aarch64" ;;
    *) echo "seek: unsupported architecture: $ARCH" >&2; exit 1 ;;
esac

ARTIFACT="seek-${OS}-${ARCH}.tar.gz"

# Determine version from git tag, fall back to latest
VERSION="$(cd "$INSTALL_DIR" && git describe --tags --exact-match 2>/dev/null || echo "")"

if [ -n "$VERSION" ]; then
    DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARTIFACT}"
else
    DOWNLOAD_URL="https://github.com/${REPO}/releases/latest/download/${ARTIFACT}"
    VERSION="latest"
fi

echo "Installing seek ${VERSION}..."

# Download and extract
mkdir -p "$INSTALL_DIR/bin"
echo "Downloading $ARTIFACT..."
curl -sL "$DOWNLOAD_URL" | tar xz -C "$INSTALL_DIR/bin"
chmod +x "$INSTALL_DIR/bin/tmux-seek"

# Copy default patterns if user doesn't have one
CONFIG_DIR="${HOME}/.config/tmux-seek"
if [ ! -f "$CONFIG_DIR/patterns.yaml" ]; then
    mkdir -p "$CONFIG_DIR"
    cp "$INSTALL_DIR/patterns.yaml" "$CONFIG_DIR/patterns.yaml"
    echo "Installed default patterns to $CONFIG_DIR/patterns.yaml"
fi

echo "seek installed to $INSTALL_DIR/bin/seek"
