# hajのbash補完。
#
# zsh版と同じく候補を持たない。コアの `haj __complete` に聞くだけ。
# bashは説明文を表示できないので、名前(タブより前)だけを使う。

_haj_complete() {
  local cur cands
  cur="${COMP_WORDS[COMP_CWORD]}"

  if [ "$COMP_CWORD" -eq 1 ]; then
    cands="$(haj __complete 2>/dev/null | cut -f1)"
  else
    # そのコマンド以降の、カーソル直前までの入力済みの語を渡す
    local sub="${COMP_WORDS[1]}"
    local words=("${COMP_WORDS[@]:2:COMP_CWORD-2}")
    cands="$(haj __complete "$sub" "${words[@]}" 2>/dev/null)"
    # 丸括弧だけの説明行は候補ではない(SPEC.md 4.3)
    case "$cands" in
      \(*\)) return 0 ;;
    esac
  fi

  mapfile -t COMPREPLY < <(compgen -W "$cands" -- "$cur")
}

complete -F _haj_complete haj
