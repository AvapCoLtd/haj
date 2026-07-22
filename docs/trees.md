# ツリーの作り方と配布(haj tree)

コマンド・エイリアス・ドキュメントの集まり(=ツリー)を git リポジトリで配る。
**パッケージマネージャではない** — clone したディレクトリが探索の対象になるだけ。

## 使う側

```sh
haj tree install https://github.com/you/haj-tools     # 入れる(@<ref> でブランチ/タグ固定)
haj tree update                                        # 差分(git log)を見せてから更新
haj tree list
haj tree remove haj-tools
haj tree configure haj-tools                           # ツリーの初期値提案を確認して取り込む
```

- 置き場は `~/.local/share/haj/trees/<名前>`。状態ファイルは持たない —
  **gitのリポジトリ自体が状態**(URLはremote、版はHEAD)
- 一覧・`haj which` には `[<ツリー名>]` として出る
- update は ff-only で、黙って入れ替えない(先に `git log --oneline 旧..新` を見せる)
- `configure` はツリーの `config-init`(あれば)を実行し、提案された設定を
  **表示 → y/N → ユーザー設定へ追記**する。既にある鍵は追記しない(既存の値を
  優先)。ツリーの設定が勝手に効くことは無い — 保存先は常にユーザー設定
  (書くことが同意)

イメージに焼くなら `--global`(`/usr/local/share/haj/trees` に入る):

```dockerfile
RUN haj tree install --global https://github.com/you/haj-tools
```

Dockerfileで `COPY` して同じ場所に置いても等価に動く(installはcloneしているだけ)。

## 作る側

リポジトリの形は2つ。**`.haj/` があればそれ、無ければルートがツリー**。

```
haj-tools/                # 配布専用の形(ルート直下に置く)
├── config                # name と alias.*(任意)
├── commands/             # 実行可能ファイル(任意)
│   ├── deploy
│   └── seed
├── docs/                 # haj docs に載るmarkdown(任意)
│   ├── quickref.md       # 圧縮リファレンス。haj help --quick が全ツリー分を連結(任意)
│   └── onboarding.md
├── lib/                  # 共通ライブラリ。コマンドから $HAJ_ROOT/lib/... で読む(任意)
├── config-init           # tree.* 設定の初期値提案(実行ファイル・任意)
├── help.header           # haj help の先頭に出す案内(任意)
└── help.footer           # 同・末尾(任意)
```

- `commands/` `docs/` `config` は**どれも任意**だが、全部無いと install は拒否される
- **エイリアス集だけの配布**も正当: `config` に `alias.*` を並べるだけで、
  1行の委譲(scripts 相当)をチームに配れる
- `docs/*.md` は `haj docs` の一覧に出自付きで載る。一覧の説明は
  **ファイル先頭の見出し行**(`# タイトル`)から取られる
- `config-init` は `haj tree configure` から実行され、stdout に
  `env.KEY = 値` / `secret.KEY = 参照` の行(と `#` コメント・空行)を出す。
  **インストール名は書かない** — コアが追記時に `tree.<インストール名>.` を
  付ける(多重インストール対応)。環境には `HAJ_ROOT` / `HAJ_TREE` /
  `HAJ_USER_CONFIG` が入る。規約フックではないので時間のかかる個人化
  (金庫 CLI で自分のユーザー名を引く等)をしてよい。静的な提案で足りる
  ツリーは `#!/bin/sh` + `cat <<EOF` の数行で済む。
  本人についての値は `haj config get meta.username` を先に見るのが定石
  (SPEC §8.5 — 無ければ検出し、`haj config set meta.username <名前>` で
  固定できると stderr で案内する)
- `docs/quickref.md` は**例文中心・1コマンド1〜2行**の圧縮リファレンス。
  `haj help --quick` がコアと全ツリー分を `## <インストール名>` 見出しで連結して
  出す(エディタや AI セッションに「いま何ができるか」を一枚で渡す出口)。
  コマンド例は namespace ツリーなら **`haj {TREE} <名前> ...`** と書く —
  `{TREE}` は出力時にインストール名へ置換される。flat ツリーは素の
  `haj <名前> ...` を書く。見出しはコアが付けるので書かない
- コマンドの書き方そのものは `haj docs writing-commands`

**配布物は自分の名前を知らない。** インストール名は使用者が選ぶ(`--name`)ので、
ツリーのコード・config-init・quickref にインストール名を書き込んではならない。
実行時は `HAJ_TREE`(SPEC §3.1)、config-init は鍵の裸書き(コアが前置)、
quickref は `{TREE}` プレースホルダ — それぞれの層に同じ原則の口がある。

```
# config の例
name = haj-tools
alias.pj = -C ~/repos/main-project
alias.pj.desc = メインプロジェクトを起点に実行する
```

## 名前空間(2段構えの語彙)

ツリー名はそのまま**名前空間**になる: `haj <ツリー名> <名前>` はそのツリーの
コマンドを明示的に呼ぶ(`haj <ツリー名>` で一覧)。これはどのツリーでも常に使える。

install / update のような**汎用動詞**を配るなら、config に `expose = namespace` を
宣言する。コマンドが素の探索から外れ、名前空間経由でだけ呼べるようになる —
素の語彙を汚さず、`haj ext install` の1行はコアの動詞とも紛れない。

```
# config — 汎用動詞を配るツリー
name = ext
expose = namespace
```

- 既定は `flat`(従来どおり素の探索にも乗る)。`cert` のように名前がそのまま
  意味になるコマンド群は既定のままでよい
- `expose` が効くのは commands だけ。docs とエイリアスは従来どおり探索に乗る
- 使い方は `haj help <ツリー名> <名前>`、環境変数は `haj env <ツリー名> <名前>`
- 配るコマンドは接続先などの設定値をハードコードせず、`VAR="${VAR:-既定値}"` で
  環境変数に昇格して `--haj-env` で申告する(writing-commands §3)。
  `haj env <ツリー名>`(名前なし)で全コマンドの実効値が一覧できる状態を保つ

## 秘密は宣言 + pull(tree.* と haj secret / store)

ツリーのコマンドは秘密を env で受け取らない。**宣言**(ユーザー設定の
`tree.<インストール名>.secret.KEY = <参照>` / `.template.KEY = <tplパス>`)を
目録として、**要る瞬間に引く**:

```sh
token=$(haj secret get MM_TOKEN)                   # 宣言を解決 (宣言に無い KEY はエラー)
keyfile=$(haj secret file OCI_KEY)                 # ファイル前提の CLI へはパスで
printf '%s' "$refresh" | haj store put token       # 実行時に得た秘密は自ツリーの store へ
```

- 平文の設定は `tree.<名前>.env.KEY`(exec 時に注入。展開しない)
- bao へのログインはコアの連鎖(`secrets.vault_cert_login` → `vault_login`)が
  拾う。コマンド側の定石は `haj store status >/dev/null 2>&1 || haj store login >&2`
  の1行(直接 bao を叩く前に)。参照の解決経由なら1行も要らない
- 初期値は `config-init` で提案し、検証は `haj secret check --tree <名前>` /
  `haj config --tree <名前>`

## 外部 CLI のラッパー(5段の型)

`oci` / `glab` のような既存 CLI を「資格情報を宣言から引いて」起動するラッパーは、
この型に収まる:

```sh
#!/bin/sh
case "${1:-}" in
  --haj-describe) echo "外部CLIをツリー宣言の資格情報で起動する"; exit 0 ;;   # 1. 説明
  --haj-help)     cat <<'EOF'                                                # 2. 使い方
...
EOF
    exit 0 ;;
  --haj-complete) shift; printf '@delegate\tocic\n'; exit 0 ;;              # 3. 補完は本人へ委譲
  --haj-env)      printf 'OCI_PROFILE=%s\n' "${OCI_PROFILE:-DEFAULT}"; exit 0 ;;  # 4. 設定の申告
esac
KEY_FILE=$(haj secret file OCI_KEY) || exit 1                                # 5. 宣言を pull して exec
export OCI_CLI_KEY_FILE="$KEY_FILE"
exec oci "$@"
```

補完の `@delegate`(SPEC §6)で、ラップしても元 CLI の補完がそのまま効く。
`--secret-file` を並べたエイリアスからの移行は、この型への置き換えで完了する。

## ツリーの分類(会社固有 vs 汎用)

- **会社・組織の文脈を持つもの**(接続先やロールが組織固有)は、その名前を
  名乗るツリーに置き、`expose = namespace` で会社文脈を呼び出しの1行に明示する
- **汎用の道具**(どの環境でも同じ意味)は汎用名のツリーへ。install / update の
  ような汎用動詞を含むなら、これも `expose = namespace`
- 迷ったら「このコマンド例を社外の README に貼れるか」— 貼れないなら会社ツリー

## 多重インストール(インスタンス)

同じツリーを別名で2つ入れて、アカウントや環境を使い分けられる:

```sh
haj tree install <URL> --name work
haj tree install <URL> --name home
```

コマンドには**インストール名**が `HAJ_TREE` として渡る(SPEC §3.1)。ローカル状態
(取得したトークンなど)の置き場はこれで分けること — 固定パスに書くと、2つの
インスタンスが同じファイルを奪い合う:

```sh
state_dir="${XDG_STATE_HOME:-$HOME/.local/state}/myext/${HAJ_TREE:-default}"
```

## 優先順位と素性

同名があれば **プロジェクト > 個人 > インストール済みツリー > HAJ_COMMAND_PATH > PATHのhaj-*** の
先勝ち。名前の位置全体では **予約語 > エイリアス > ツリー名前空間 > この探索** の順。
どれが効いているかは `haj which --all <名前>` で常に確認できる。

install は URL を自分で打つ行為なので、それ自体が信頼の表明。clone した中身が
haj 経由で走ることは `.haj/commands/` を持つリポジトリと同じなので、
知らないツリーを入れるときは中身を読むこと。

仕様の全文は `haj docs spec` の §9.5。
