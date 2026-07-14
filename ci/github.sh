#!/bin/sh
# GitHub ミラーと公開リリース(Issue #10 フェーズ2)。
#
#   ci/github.sh mirror            master とタグを AvapCoLtd/haj へ push する
#   ci/github.sh release <タグ>    GitHub Release を作り、dist-public/* を添付する
#
# 認証は GitHub App「haj-release」(Contents RW / haj のみ)。CI 変数で受け取る:
#   GH_APP_ID           App ID
#   GH_APP_PRIVATE_KEY  秘密鍵(file 型変数。値は bao の users/hajime/github-app/haj-release)
#
# App の installation token は1時間で失効するので、ジョブのたびに発行する。
# 依存は openssl / curl / git だけ(jq は使わない。JSONは素朴に抜く)。
set -eu

REPO="${GH_REPO:-AvapCoLtd/haj}"
API="https://api.github.com"

die() { echo "github.sh: $*" >&2; exit 1; }

[ -n "${GH_APP_ID:-}" ] || die "GH_APP_ID がありません(CI変数を設定してください)"
[ -n "${GH_APP_PRIVATE_KEY:-}" ] || die "GH_APP_PRIVATE_KEY がありません(file型のCI変数)"
[ -f "$GH_APP_PRIVATE_KEY" ] || die "GH_APP_PRIVATE_KEY は file 型で渡してください(パスが $GH_APP_PRIVATE_KEY)"

b64url() { openssl base64 -A | tr '+/' '-_' | tr -d '='; }

# App の秘密鍵で JWT を署名し、installation token(1h)を発行する。
token() {
  now=$(date +%s)
  header=$(printf '{"alg":"RS256","typ":"JWT"}' | b64url)
  payload=$(printf '{"iat":%d,"exp":%d,"iss":"%s"}' $((now - 60)) $((now + 540)) "$GH_APP_ID" | b64url)
  sig=$(printf '%s.%s' "$header" "$payload" | openssl dgst -sha256 -sign "$GH_APP_PRIVATE_KEY" | b64url)
  jwt="$header.$payload.$sig"

  inst=$(curl -fsSL -H "Authorization: Bearer $jwt" "$API/repos/$REPO/installation" \
    | sed -n 's/.*"id": *\([0-9]*\).*/\1/p' | head -1)
  [ -n "$inst" ] || die "installation が見つかりません。App が $REPO にインストールされていますか"

  curl -fsSL -X POST -H "Authorization: Bearer $jwt" \
    "$API/app/installations/$inst/access_tokens" \
    | sed -n 's/.*"token": *"\([^"]*\)".*/\1/p' | head -1
}

cmd="${1:-}"
case "$cmd" in
mirror)
  t=$(token)
  [ -n "$t" ] || die "installation token を発行できません"
  url="https://x-access-token:${t}@github.com/${REPO}.git"
  # GitLab が正(canonical)。ミラーは常に GitLab の姿へ合わせる。
  if [ -n "${CI_COMMIT_TAG:-}" ]; then
    echo "==> タグ ${CI_COMMIT_TAG} を ${REPO} へ"
    git push --force "$url" "refs/tags/${CI_COMMIT_TAG}"
  else
    echo "==> ${CI_COMMIT_BRANCH:-master} を ${REPO} へ"
    git push --force "$url" "HEAD:refs/heads/${CI_COMMIT_BRANCH:-master}"
  fi
  ;;

release)
  tag="${2:?使い方: github.sh release <タグ>}"
  t=$(token)
  [ -n "$t" ] || die "installation token を発行できません"
  auth="Authorization: token $t"

  echo "==> Release $tag を作成します"
  body="haj ${tag#v} — 詳細は SPEC.md / haj docs を参照。"
  upload_url=$(curl -fsSL -X POST -H "$auth" "$API/repos/$REPO/releases" \
    -d "{\"tag_name\":\"$tag\",\"name\":\"$tag\",\"body\":\"$body\"}" \
    | sed -n 's/.*"upload_url": *"\([^"{]*\).*/\1/p' | head -1)
  [ -n "$upload_url" ] || die "Release を作成できません(既に存在する場合は先に消してください)"

  for f in dist-public/*; do
    [ -f "$f" ] || continue
    name=$(basename "$f")
    echo "==> アセット $name"
    curl -fsSL -X POST -H "$auth" -H "Content-Type: application/octet-stream" \
      --data-binary "@$f" "${upload_url}?name=${name}" >/dev/null
  done
  echo "==> https://github.com/$REPO/releases/tag/$tag"
  ;;

*)
  die "使い方: github.sh <mirror|release>"
  ;;
esac
