#!/bin/sh
# hajをインストールする。
#
#   HAJ_TOKEN=<GitLabトークン> ./install.sh           最新のリリースを入れる
#   HAJ_TOKEN=<GitLabトークン> ./install.sh 0.2.0     版を指定して入れる
#
# このリポジトリはprivateなので、Package Registryからの取得にトークンが要る。
# 使えるのは Personal Access Token (read_api) / Project Access Token / Deploy Token。
# CI の中からなら CI_JOB_TOKEN でよい(同一GitLabインスタンス内に限る)。
#
# CIコンテナやイメージのビルドから使うことを想定して POSIX sh で書いてある。
# bash も curl 以外の依存も要らない。
set -eu

GITLAB="${HAJ_GITLAB:-https://gitlab.avaper.day}"
PROJECT_ID="${HAJ_PROJECT_ID:-788}"
PREFIX="${HAJ_PREFIX:-/usr/local}"
TARGET="${HAJ_TARGET:-x86_64-unknown-linux-musl}"

die() { echo "install.sh: $*" >&2; exit 1; }

command -v curl >/dev/null 2>&1 || die "curl が必要です"
command -v tar  >/dev/null 2>&1 || die "tar が必要です"

[ -n "${HAJ_TOKEN:-}" ] || die "HAJ_TOKEN を設定してください (GitLabのトークン)。
  例: HAJ_TOKEN=glpat-xxxx ./install.sh"

auth_header() {
  # CI_JOB_TOKEN は JOB-TOKEN ヘッダ、それ以外は PRIVATE-TOKEN ヘッダ。
  if [ -n "${CI_JOB_TOKEN:-}" ] && [ "$HAJ_TOKEN" = "${CI_JOB_TOKEN:-}" ]; then
    printf 'JOB-TOKEN: %s' "$HAJ_TOKEN"
  else
    printf 'PRIVATE-TOKEN: %s' "$HAJ_TOKEN"
  fi
}

VERSION="${1:-}"
if [ -z "$VERSION" ]; then
  echo "==> 最新のリリースを調べています"
  # releases APIの先頭が最新。jqに依存したくないので素朴に抜き出す。
  VERSION=$(curl -fsSL --header "$(auth_header)" \
      "${GITLAB}/api/v4/projects/${PROJECT_ID}/releases?per_page=1" \
    | sed -n 's/.*"tag_name":"\([^"]*\)".*/\1/p' | head -1 | sed 's/^v//')
  [ -n "$VERSION" ] || die "リリースが見つかりません。版を明示してください: ./install.sh 0.1.0"
fi

ASSET="haj-${TARGET}.tar.gz"
URL="${GITLAB}/api/v4/projects/${PROJECT_ID}/packages/generic/haj/${VERSION}/${ASSET}"

echo "==> haj ${VERSION} (${TARGET}) を取得します"
TMP=$(mktemp -d)
# どの経路で抜けてもtmpを消す。鍵は置かないが、途中で落ちたゴミを残さない。
trap 'rm -rf "$TMP"' EXIT INT TERM

curl -fsSL --header "$(auth_header)" -o "$TMP/$ASSET" "$URL" \
  || die "取得に失敗しました: $URL
  版が存在しないか、トークンに read_api / read_package_registry の権限がありません。"

# 改竄と転送事故の検出。CIが一緒に上げている .sha256 と突き合わせる。
if curl -fsSL --header "$(auth_header)" -o "$TMP/${ASSET}.sha256" "${URL}.sha256" 2>/dev/null; then
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

# シェル補完。あれば入れる(無くても haj は動くので失敗させない)。
SRC=$(dirname "$0")
ZSH_DIR="${PREFIX}/share/zsh/site-functions"
if [ -f "$SRC/completions/_haj" ] && [ -d "$ZSH_DIR" ] && [ -w "$ZSH_DIR" ]; then
  install -m 644 "$SRC/completions/_haj" "$ZSH_DIR/_haj"
  echo "==> zsh補完を ${ZSH_DIR}/_haj に入れました"
fi
