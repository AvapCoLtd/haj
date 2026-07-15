# ツリーの作り方と配布(haj tree)

コマンド・エイリアス・ドキュメントの集まり(=ツリー)を git リポジトリで配る。
**パッケージマネージャではない** — clone したディレクトリが探索の対象になるだけ。

## 使う側

```sh
haj tree install https://github.com/you/haj-tools     # 入れる(@<ref> でブランチ/タグ固定)
haj tree update                                        # 差分(git log)を見せてから更新
haj tree list
haj tree remove haj-tools
```

- 置き場は `~/.local/share/haj/trees/<名前>`。状態ファイルは持たない —
  **gitのリポジトリ自体が状態**(URLはremote、版はHEAD)
- 一覧・`haj which` には `[<ツリー名>]` として出る
- update は ff-only で、黙って入れ替えない(先に `git log --oneline 旧..新` を見せる)

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
│   └── onboarding.md
├── lib/                  # 共通ライブラリ。コマンドから $HAJ_ROOT/lib/... で読む(任意)
├── help.header           # haj help の先頭に出す案内(任意)
└── help.footer           # 同・末尾(任意)
```

- `commands/` `docs/` `config` は**どれも任意**だが、全部無いと install は拒否される
- **エイリアス集だけの配布**も正当: `config` に `alias.*` を並べるだけで、
  1行の委譲(scripts 相当)をチームに配れる
- `docs/*.md` は `haj docs` の一覧に出自付きで載る。一覧の説明は
  **ファイル先頭の見出し行**(`# タイトル`)から取られる
- コマンドの書き方そのものは `haj docs writing-commands`

```
# config の例
name = haj-tools
alias.pj = -C ~/repos/main-project
alias.pj.desc = メインプロジェクトを起点に実行する
```

## 優先順位と素性

同名があれば **プロジェクト > 個人 > インストール済みツリー > HAJ_COMMAND_PATH > PATHのhaj-*** の
先勝ち。どれが効いているかは `haj which --all <名前>` で常に確認できる。

install は URL を自分で打つ行為なので、それ自体が信頼の表明。clone した中身が
haj 経由で走ることは `.haj/commands/` を持つリポジトリと同じなので、
知らないツリーを入れるときは中身を読むこと。

仕様の全文は `haj docs spec` の §9.5。
