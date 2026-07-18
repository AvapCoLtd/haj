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
    mapfile -t COMPREPLY < <(compgen -W "$cands" -- "$cur")
    return 0
  fi

  # サブコマンド以降。入力済みの語を(フラグ込みで)素通しで core へ。
  local words=("${COMP_WORDS[@]:1:COMP_CWORD-1}")
  local out first
  out="$(haj __complete "${words[@]}" 2>/dev/null)"
  first="${out%%$'\n'*}"

  # 1行目の @ 始まりはシェルへの指示(SPEC.md 4.3, §6)
  case "$first" in
    '@files'|$'@files\t'*|'@dirs')
      # ファイル補完の指示:
      #   @files                → ファイルとディレクトリ
      #   @files<TAB><glob>...  → glob(タブ区切り、shの書式)に合うファイルだけ
      #   @dirs                 → ディレクトリのみ
      # 指示行の後に続く行は、通常の候補として併せて出す
      local f g
      if [ "$first" = '@dirs' ]; then
        mapfile -t COMPREPLY < <(compgen -d -- "$cur")
      else
        local -a globs=()
        if [ "$first" != '@files' ]; then
          IFS=$'\t' read -r -a globs <<<"$first"
          globs=("${globs[@]:1}")
        fi
        while IFS= read -r f; do
          [ -n "$f" ] || continue
          if [ -d "$f" ] || [ "${#globs[@]}" -eq 0 ]; then
            COMPREPLY+=("$f")
          else
            # glob はファイル名(パスの最後の要素)に対して照合する
            for g in "${globs[@]}"; do
              case "${f##*/}" in $g) COMPREPLY+=("$f"); break ;; esac
            done
          fi
        done < <(compgen -f -- "$cur")
      fi
      compopt -o filenames 2>/dev/null
      # 指示行の後の行は通常の候補(名前だけ使う)
      local rest
      rest="$(printf '%s\n' "$out" | tail -n +2 | cut -f1)"
      if [ -n "$rest" ]; then
        local -a extra=()
        mapfile -t extra < <(compgen -W "$rest" -- "$cur")
        COMPREPLY+=("${extra[@]}")
      fi
      return 0 ;;
    '@'*)
      # @delegate と未知の指示は候補として表示しない(SPEC.md §6)。
      # bash では他コマンドの補完へ安全に委譲できないので候補なし
      return 0 ;;
  esac

  # bashは説明文を表示できないので、"名前<TAB>説明" の行は名前だけ使う
  cands="$(printf '%s\n' "$out" | cut -f1)"
  # 丸括弧だけの説明行は候補ではない(SPEC.md 4.3)
  case "$cands" in
    \(*\)) return 0 ;;
  esac

  mapfile -t COMPREPLY < <(compgen -W "$cands" -- "$cur")
}

complete -F _haj_complete haj
