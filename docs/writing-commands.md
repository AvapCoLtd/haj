# コマンドの作り方

haj のコマンドは**ただの実行可能ファイル**である。登録も宣言も要らない。
置けば生え、消せば消える。言語は問わない(シェルでも Rust でも Python でもよい)。

## 1. 最小のコマンド(3行)

```sh
mkdir -p .haj/commands
printf '#!/bin/sh\necho hello "$1"\n' > .haj/commands/greet
chmod +x .haj/commands/greet
```

これで、このリポジトリの中ならどこでも `haj greet world` が動く。

- **実行ビットが立った通常ファイル**だけがコマンドになる(README や .gitkeep は無視される)
- 名前に使えないもの: `.` / `-` 始まり、`/` を含むもの、予約語
  (`help` `commands` `which` `config` `completion` `docs` `env` `exec` `sh` `run`
  `selfupgrade` `secrets` `tree` `__complete`)

## 2. 置き場所と優先順位

| 順 | 場所 | 用途 |
|---|---|---|
| 1 | `<リポジトリ>/.haj/commands/` | プロジェクト固有 |
| 2 | `~/.config/haj/commands/` | 個人用 |
| 3 | `$HAJ_COMMAND_PATH`(既定 `/usr/local/lib/haj/commands`) | 全社/イメージ共通 |
| 4 | `$PATH` 上の `haj-<名前>` | git 方式の逃げ道 |

同名は**手前が勝つ**。プロジェクトの `build` が共通の `build` を上書きする。
どれが効いているかは `haj which --all <名前>` で確認できる。

探索は cwd から上へ遡るが、**最初に見つけた `.haj` で止まる**(プロジェクト境界)。
モノレポのサブプロジェクトで親のコマンドも継承したいときだけ、`.haj/config` に
`root = false` と書いて壁を開ける。

### PATH の `haj-<名前>`(git 方式)

`$PATH` 上に `haj-<名前>` という実行可能ファイルを置くと、ツリーに何も置かなくても
`haj <名前>` で呼べる(git が `git-foo` を `git foo` にするのと同じ)。

```sh
printf '#!/bin/sh\necho hello from PATH\n' > ~/bin/haj-hello
chmod +x ~/bin/haj-hello
haj hello
```

- 探索の**最後**(表の4番)。ツリーの同名コマンドには常に負ける
- **`HAJ_ROOT` は設定されない**(属するツリーが無い)。`$HAJ_ROOT/lib` に依存せず
  自己完結で書くこと。`HAJ_PROJECT` は cwd から決まるので普通に渡る
- 規約フック(`--haj-describe` 等)は同じように呼ばれる。実装すれば一覧にも補完にも出る
- 使いどころ: cargo install や pipx など**言語のパッケージマネージャで配る**コマンド、
  ツリー管理に乗せたくない個人ツール。チームで配るものはツリー(`$HAJ_COMMAND_PATH`)が正道

## 3. 規約フック(これを実装すると一級市民になる)

コアは中身を知らない。知りたいこと(説明・使い方・補完)はコマンド自身に聞く。

```sh
#!/bin/sh
case "${1:-}" in
  --haj-describe) echo "一行の説明"; exit 0 ;;
  --haj-help)     cat <<'EOF'
使い方: haj mig <up|down|status>

詳しい説明をここに。haj help mig で表示される。
EOF
    exit 0 ;;
  --haj-complete)
    shift
    # $# = 入力済みの語数。0 なら1語目の候補を返す($# -le 1 と書くと1つずれる)
    if [ $# -eq 0 ]; then printf '%s\n' up down status; fi
    exit 0 ;;
  --haj-env)
    # 読む環境変数を KEY=value で。`haj env <名前>` が中継し、出力はそのまま
    # --env-file に渡せる(haj env mig > env.txt → 編集 → haj --env-file env.txt mig)
    printf '%s\n' "# 対象DB" "DB_HOST=${DB_HOST:-db.staging.internal}" "LONG_TX_SEC=${LONG_TX_SEC:-60}"
    exit 0 ;;
esac

# ---- ここから本体。重い初期化はフックの後に置く(TABの速さに直結する) ----
echo "本処理"
```

フックの制約(SPEC §4.5):

- **stdin は /dev/null**。入力を待ってはならない
- **stderr は捨てられる**。タイムアウトは既定2秒 — 超えると SIGKILL
- **副作用禁止**。フックは `haj help` と TAB のたびに呼ばれる
- `--haj-describe` を実装しなくても動く(一覧の説明が空になるだけ)。ただし必須とする

### 設定値は環境変数に昇格する

接続先・ロール名・出力先パスのような**設定値を本体にハードコードしない**。
`VAR="${VAR:-既定値}"` で受けて上書き可能にし、`--haj-env` で実効値を申告する
(上の例の `DB_HOST` がこの形)。設定値を持つコマンドでは `--haj-env` は実質必須 —
`haj env <名前>` で「何がどう効くか」が見える状態を保つのは、出自ラベルや `which` と
同じ「素性は常に見える」の一部。lib 側のスクリプトも同じ形で受けると、呼び出し元が
値を差し替えられて汎用に保てる。

候補を列挙できない箇所(自由入力)では丸括弧の1行を返すと、補完はそれを説明として出す:

```sh
--haj-complete) shift; [ $# -eq 1 ] && echo "(新しいマイグレーションの名前)"; exit 0 ;;
```

パスを取る引数では、1行目に `@files` / `@dirs` を返すとシェルがファイル補完をする
(glob はタブ区切りで複数、sh の書式。指示行の後に続く行は通常の候補として併せて出る):

```sh
# transcode <src> <out_dir> — 1語目は動画ファイル、2語目は出力先ディレクトリ
--haj-complete) shift
  case $# in
    0) printf '@files\t*.mp4\t*.mkv\t*.mov\n' ;;
    1) echo '@dirs' ;;
  esac
  exit 0 ;;
```

## 4. コアから渡されるもの

| 変数 | 意味 |
|---|---|
| `HAJ_ROOT` | このコマンドが属するツリー(`commands/` の親)。**共通ライブラリはここから読む** |
| `HAJ_NAME` | 呼ばれた名前 |
| `HAJ_PROJECT` / `HAJ_PROJECT_DIR` | いま操作対象のプロジェクト。**破壊的なコマンドは対象を名乗ること** |

```sh
. "${HAJ_ROOT:?hajルーター経由で実行してください}/lib/common.sh"
echo "==> ${HAJ_PROJECT}: セットアップします (${HAJ_PROJECT_DIR})"
```

`HAJ_ROOT`(どこから来たか)と `HAJ_PROJECT`(どこに対して実行しているか)は別物。
共通ツリーの `mig` をプロジェクトの中で叩けば、前者は共通ツリー、後者はそのプロジェクトになる。

## 5. シークレットの受け取り方

コマンド側は**普通に環境変数を読むだけ**でよい。参照の解決は haj がやる(SPEC §10)。

```sh
# 呼ぶ側:
haj --secret DB_PASS=vault://secret/data/db/password mig up
# コマンド側は $DB_PASS を読むだけ。bao の存在を知らなくてよい
```

- 渡し方は `--secret` / `--env-file` / `--secret-file`(いずれもコマンド名の前に書く)。
  haj は環境を勝手に走査しない — **人が明示的に渡したものだけ**が展開される
- 解決に失敗するとコマンドは**実行されない**(fail-fast)。未解決の参照文字列が渡ってくる心配はしなくてよい
- ファイルで渡せと要求するツール(ssh の鍵、kubeconfig など)には `--secret-file`。
  `--secret-file KEY=vault://...` なら一時ファイルに書かれ、パスが `$KEY` に入る

## 6. タスク(`haj run`)— リポジトリの作業動詞

install / update / build / test のような「このリポジトリの作業」は、コマンドではなく
**タスク**にする。プロジェクトに `install` コマンドを置くと `haj install` がコアの動詞
(`selfupgrade` / `tree install`)に見えて紛らわしい — `haj run install` なら文脈なしの
1行でも読み違えない。

置き場所は2つ(エイリアスとコマンドの関係と同型):

```
# .haj/config — 1行の委譲
task.test = exec docker compose exec app vendor/bin/phpunit
task.test.desc = テストを流す(コンテナ内)
```

```sh
# .haj/tasks/<名前> — ロジックを持つ実行ファイル(規約フックはコマンドと同じ)
mkdir -p .haj/tasks
$EDITOR .haj/tasks/install && chmod +x .haj/tasks/install
```

- `haj run` で一覧、`haj run <名前> [引数...]` で実行
- **探索しない・上書きしない・フォールバックしない。** 見るのは現在のプロジェクトの
  `task.<名前>` と `.haj/tasks/<名前>` だけ(親へも遡らない)。素の `haj <名前>` では
  呼べないし、タスクが他プロジェクトや共通のコマンドに化けることもない
- 1行で書けなくなったら `tasks/` の実行ファイルに昇格する(委譲は宣言、ロジックは実行ファイル)
- 規約フック(§3)と `HAJ_ROOT` / `HAJ_PROJECT`(§4)はコマンドと同じ。使い方は
  `haj help run <名前>`、環境変数は `haj env run <名前>`、効いている定義は
  `haj which run <名前>` で確かめられる

**使い分け**: リポジトリの作業動詞 → タスク。haj の語彙を増やすツール(`mig` のような、
名前がそのまま意味になるもの)→ コマンド。迷ったら「素の `haj <名前>` で読んだとき、
コアの動詞や他プロジェクトの同名と紛れるか?」— 紛れるならタスク。

## 7. デバッグ

```sh
haj which --all setup     # どの定義が勝っていて、何が隠れているか
haj commands              # 機械可読の一覧(名前/パス/出自/説明)
HAJ_NO_CACHE=1 haj help   # 説明文キャッシュを無効化して聞き直す
HAJ_HOOK_TIMEOUT_MS=10000 haj help   # 遅いフックの調査
haj -C ~/repos/x help     # 別プロジェクトを起点に見る
```

「説明が一覧に出ない」ときは、まず `HAJ_NO_CACHE=1` で聞き直し、次に
`.haj/commands/<名前> --haj-describe` を直接叩いて、1行目に説明が出て exit 0 かを確かめる。

## 8. やってはいけないこと

- **予約語の名前でコマンドを置く** — 無視される(`exec` や `sh` を奪えるとシークレットの横取りができてしまうため)
- **フックで対話やネットワークを待つ** — TAB のたびに全員が待たされ、2秒で殺される
- **コマンド一覧を help.header に手書きする** — 一覧は自動生成される。手書きは必ず実態とズレる
- **設定値のハードコード** — 接続先やパスは `VAR="${VAR:-既定値}"` で環境変数に昇格し、
  `--haj-env` で申告する(§3)。埋め込むと `haj env` に映らず、差し替えもできない
- **`haj.toml` 的な宣言でタスクを書きたくなる** — haj は意図的にそれをやらない(SPEC §11)。分岐や冪等性を含む現実のタスクは、最初からスクリプトとして書く

## 9. 実例

このリポジトリの `examples/.haj/commands/deploy` に、規約フック・引数解釈・
`HAJ_ROOT` の利用まで含めた実物がある。困ったら `haj docs spec` で契約の全文を引ける。
