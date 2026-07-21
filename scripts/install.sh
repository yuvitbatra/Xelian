#!/bin/sh
# Xelian installer. Downloads the prebuilt `xelian` binary for your platform
# from the latest GitHub Release and installs it — no Rust toolchain, no build.
#
#   curl -fsSL https://raw.githubusercontent.com/<owner>/<repo>/main/scripts/install.sh | sh
#
# Overrides (env):
#   XELIAN_REPO      owner/repo to install from (default below)
#   XELIAN_VERSION   tag to install (default: latest release)
#   XELIAN_BIN_DIR   install directory (default: ~/.local/bin)
#
# POSIX sh, no bashisms: this has to run under the plain /bin/sh that a
# `curl | sh` pipe uses.

set -eu

REPO="${XELIAN_REPO:-yuvitbatra/Xelian}"
BIN_DIR="${XELIAN_BIN_DIR:-$HOME/.local/bin}"

say()  { printf '\033[1;36m%s\033[0m\n' "$*"; }
warn() { printf '\033[1;33m%s\033[0m\n' "$*" >&2; }
die()  { printf '\033[1;31merror: %s\033[0m\n' "$*" >&2; exit 1; }

# --- detect platform → the target triple the release workflow builds ---------
os="$(uname -s)"
arch="$(uname -m)"
case "$os" in
  Darwin) os_part="apple-darwin" ;;
  Linux)  os_part="unknown-linux-gnu" ;;
  *) die "unsupported OS '$os' — Xelian ships macOS and Linux binaries; build from source instead" ;;
esac
case "$arch" in
  arm64|aarch64) arch_part="aarch64" ;;
  x86_64|amd64)  arch_part="x86_64" ;;
  *) die "unsupported architecture '$arch'" ;;
esac
target="${arch_part}-${os_part}"

# --- resolve version ---------------------------------------------------------
version="${XELIAN_VERSION:-}"
if [ -z "$version" ]; then
  say "Resolving the latest Xelian release..."
  version="$(
    curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
      | grep '"tag_name"' | head -1 | cut -d'"' -f4
  )" || true
  [ -n "$version" ] || die "could not find a published release for ${REPO}. Set XELIAN_VERSION, or build from source (cargo build --release)."
fi

asset="xelian-${target}.tar.gz"
url="https://github.com/${REPO}/releases/download/${version}/${asset}"

# --- download + verify -------------------------------------------------------
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

say "Downloading ${asset} (${version})..."
curl -fsSL "$url" -o "$tmp/$asset" \
  || die "download failed: $url (is there a release asset for ${target}?)"

# Verify the checksum if the release ships one (it does; best-effort if not).
if curl -fsSL "${url}.sha256" -o "$tmp/${asset}.sha256" 2>/dev/null; then
  say "Verifying checksum..."
  expected="$(cut -d' ' -f1 < "$tmp/${asset}.sha256")"
  if command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$tmp/$asset" | cut -d' ' -f1)"
  else
    actual="$(sha256sum "$tmp/$asset" | cut -d' ' -f1)"
  fi
  [ "$expected" = "$actual" ] || die "checksum mismatch — refusing to install a tampered download"
else
  warn "no checksum published for this asset; skipping verification"
fi

# --- install -----------------------------------------------------------------
tar -C "$tmp" -xzf "$tmp/$asset" || die "failed to extract archive"
[ -f "$tmp/xelian" ] || die "archive did not contain a xelian binary"

mkdir -p "$BIN_DIR"
install -m 0755 "$tmp/xelian" "$BIN_DIR/xelian" 2>/dev/null \
  || { cp "$tmp/xelian" "$BIN_DIR/xelian" && chmod 0755 "$BIN_DIR/xelian"; }

say "Installed xelian ${version} to ${BIN_DIR}/xelian"

# --- PATH hint ---------------------------------------------------------------
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    warn "${BIN_DIR} is not on your PATH. Add it, e.g.:"
    warn "    echo 'export PATH=\"${BIN_DIR}:\$PATH\"' >> ~/.profile && . ~/.profile"
    ;;
esac

printf '\n\033[1;32mRun `xelian --help` to get started, or:\033[0m\n'
printf '    xelian add https://github.com/zcaceres/fetch-mcp\n'
