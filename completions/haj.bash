# hajのbash補完。
#
# zsh版と同じく候補を持たない。コアの `haj __complete` に聞くだけ。
# 入力済みの語(グローバルフラグ込み)を素通しで渡し、コアが本体と同じ規則で
# フラグを読み飛ばす(-Cは適用)。シェル側で処理するのは「フラグの値の位置」
# (ファイル/ディレクトリ補完)だけ。
# bashは説明文を表示できないので、名前(タブより前)だけを使う。

_haj_complete() {
  local cur cands
  cur="${COMP_WORDS[COMP_CWORD]}"

  # グローバルフラグを読み飛ばす。値の位置ならファイル/ディレクトリ補完
  local i=1
  while [ "$i" -lt "$COMP_CWORD" ]; do
    case "${COMP_WORDS[$i]}" in
      -C)
        if [ $((i + 1)) -eq "$COMP_CWORD" ]; then
          mapfile -t COMPREPLY < <(compgen -d -- "$cur")
          return 0
        fi
        i=$((i + 2)) ;;
      --env-file|--secret-file)
        if [ $((i + 1)) -eq "$COMP_CWORD" ]; then
          mapfile -t COMPREPLY < <(compgen -f -- "$cur")
          return 0
        fi
        i=$((i + 2)) ;;
      --secret)
        [ $((i + 1)) -eq "$COMP_CWORD" ] && return 0
        i=$((i + 2)) ;;
      *) break ;;
    esac
  done

  if [ "$i" -eq "$COMP_CWORD" ]; then
    # コマンド名の位置。`-` で始めたらグローバルフラグを出す
    case "$cur" in
      -*) cands="-C --secret --env-file --secret-file" ;;
      *)  cands="$(haj __complete "${COMP_WORDS[@]:1:COMP_CWORD-1}" 2>/dev/null | cut -f1)" ;;
    esac
  else
    # サブコマンド以降。入力済みの語を(フラグ込みで)素通しで core へ。
    # bashは説明文を表示できないので、"名前<TAB>説明" の行は名前だけ使う
    local words=("${COMP_WORDS[@]:1:COMP_CWORD-1}")
    cands="$(haj __complete "${words[@]}" 2>/dev/null | cut -f1)"
    # 丸括弧だけの説明行は候補ではない(SPEC.md 4.3)
    case "$cands" in
      \(*\)) return 0 ;;
    esac
  fi

  mapfile -t COMPREPLY < <(compgen -W "$cands" -- "$cur")
}

complete -F _haj_complete haj
