#!/bin/sh
# haj を GitHub Releases から入れる(トークン不要)。
#
#   curl -fsSL https://raw.githubusercontent.com/AvapCoLtd/haj/master/install.sh | sh
#   ./install.sh 0.11.0     版を指定して入れる
#
# 依存ゼロの静的バイナリなので、glibc も bash も要らない。
set -eu

REPO="${HAJ_REPO:-AvapCoLtd/haj}"
PREFIX="${HAJ_PREFIX:-/usr/local}"

die() { echo "install.sh: $*" >&2; exit 1; }

command -v curl >/dev/null 2>&1 || die "curl が必要です"
command -v tar  >/dev/null 2>&1 || die "tar が必要です"

case "$(uname -s)-$(uname -m)" in
  Linux-x86_64)  TARGET="x86_64-unknown-linux-musl" ;;
  Linux-aarch64) TARGET="aarch64-unknown-linux-musl" ;;
  *) die "このプラットフォーム ($(uname -s) $(uname -m)) のビルドはまだありません。
  cargo build --release で手元からビルドしてください(依存クレートはゼロです)。" ;;
esac
TARGET="${HAJ_TARGET:-$TARGET}"

VERSION="${1:-}"
if [ -z "$VERSION" ]; then
  echo "==> 最新のリリースを調べています"
  VERSION=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -1 | sed 's/^v//')
  [ -n "$VERSION" ] || die "リリースが見つかりません。版を明示してください: ./install.sh 0.11.0"
fi

ASSET="haj-${TARGET}.tar.gz"
URL="https://github.com/${REPO}/releases/download/v${VERSION}/${ASSET}"

echo "==> haj ${VERSION} (${TARGET}) を取得します"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT INT TERM

curl -fsSL -o "$TMP/$ASSET" "$URL" || die "取得に失敗しました: $URL"

# 改竄と転送事故の検出
if curl -fsSL -o "$TMP/${ASSET}.sha256" "${URL}.sha256" 2>/dev/null; then
  if command -v sha256sum >/dev/null 2>&1; then
    ( cd "$TMP" && sha256sum -c "${ASSET}.sha256" >/dev/null ) \
      || die "チェックサムが一致しません。取得し直してください。"
    echo "==> チェックサム OK"
  fi
fi

tar xzf "$TMP/$ASSET" -C "$TMP"
[ -f "$TMP/haj" ] || die "アーカイブに haj が入っていません"

BIN="${PREFIX}/bin"
if [ -w "$BIN" ]; then
  install -m 755 "$TMP/haj" "$BIN/haj"
elif command -v sudo >/dev/null 2>&1; then
  sudo install -m 755 "$TMP/haj" "$BIN/haj"
else
  die "$BIN に書けません。HAJ_PREFIX=\$HOME/.local などを指定してください。"
fi

echo "==> $($BIN/haj --version) を ${BIN}/haj に入れました"
