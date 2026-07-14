# haj

**h**ack **a**pplication **j**ob — プロジェクトごとに中身が変わるジョブランナー。

`haj` はサブコマンドを**持たない**。そこに置いてある実行可能ファイルを**探して**実行する。
だから、リポジトリごとに使えるコマンドが違う、という状態が自然に成立する。

```console
$ cd ~/repos/webapp && haj
 hajコマンド (haj help <名前> で詳細):
   web        基本版(webapp)の操作
   ie         拡張版(example-app)の操作
   mig        DBマイグレーション (status/up/down/create/edit)
   xdebug     Xdebugの有効/無効を切り替え

$ cd ~/repos/some-other-project && haj
 hajコマンド (haj help <名前> で詳細):
   deploy     このプロジェクトのデプロイ
   seed       テストデータ投入
```

同じ `haj` コマンドだが、出てくるものが違う。**リポジトリに置いたコマンドだけが、
そのリポジトリで生える。**

## なぜ make / just / npm scripts ではないのか

- **一覧が信用できる。** `haj help` はコマンドを1本ずつ叩いて説明を集める。手で書いた
  一覧が実態と食い違う、ということが起こらない。
- **補完が勝手についてくる。** サブコマンドが `--haj-complete` に答えれば、TAB 補完に
  自動で載る。補完スクリプトを書き足す必要がない。
- **タスクを宣言型ファイルに閉じ込めない。** 現実のタスクは分岐と冪等性判定の塊で、
  TOML/YAML の1行には収まらない。haj のタスクは**普通の実行ファイル**なので、
  シェルでも Rust でも PHP でも書ける。
- **共通コマンドと固有コマンドが両立する。** 全社共通の `bao-login` は全リポジトリで
  使えて、`deploy` はそのリポジトリでだけ生える。同名なら手前が勝つ。

## インストール

Linux x86_64 / aarch64 の**静的バイナリ**(musl)。glibc も bash も要らない(alpine でも動く)。

```sh
curl -fsSL https://raw.githubusercontent.com/AvapCoLtd/haj/master/install.sh | sh
```

- 既定で `/usr/local/bin/haj` に入る(書けなければ sudo を使う)
- 別の場所に入れる: `curl -fsSL ... | HAJ_PREFIX=$HOME sh` → `~/bin/haj`
- 版を指定する: `curl -fsSL ... -o install.sh && sh install.sh 0.13.0`

インストーラはアーキテクチャを自動で判別し、`.sha256` で改竄と転送事故を検出する。

手で入れる場合、または他のプラットフォーム(macOS など)は、リリースの tar.gz を
展開するか、手元でビルドする。依存クレートはゼロなので Rust さえあれば通る。

```sh
cargo build --release && install -m 755 target/release/haj /usr/local/bin/haj
```

## シェル補完

```sh
# ~/.zshrc
eval "$(haj completion zsh)"

# ~/.bashrc
eval "$(haj completion bash)"
```

補完スクリプトは候補を一切持たない。`haj __complete` でコアに聞くだけなので、
**コマンドを足しても更新は要らない**(プロジェクト固有のコマンドもそのまま候補に出る)。

## 更新

```sh
haj selfupgrade            # 最新版に入れ替える(最新なら何もしない)
haj selfupgrade --check    # 調べるだけ (0=最新 / 1=更新あり / 2=調べられず)
haj selfupgrade 0.12.1     # 版を指定(ダウングレードもこれ)
```

GitHub Releases から取ってくる。**認証は要らない**。置き換えは同じディレクトリに書いて
から `rename(2)` するので原子的で、実行中のプロセスは壊れない。書けない場所なら sudo での
再実行を提案して終わる(コア自身は昇格しない)。

private な取得元(社内の GitLab など)から更新するなら、`~/.config/haj/config` に書く:

```
selfupgrade.gitlab     = https://gitlab.example.com
selfupgrade.project_id = 123
selfupgrade.token      = vault://<マウント>/<パス>/token   # 平文でもよい
```

`selfupgrade.gitlab` を設定したときだけ GitLab の Package Registry を見る。トークンは
**シークレット参照で書ける**ので、平文をディスクに置かずに済む。

## 使い方

```
haj <コマンド> [引数...]         探索して実行する
haj                            プロジェクトと、使えるコマンドの一覧
haj help <名前>                 そのコマンドの詳しい使い方
haj which [--all] <名前>        どの定義が効いているかを見る
haj commands                   一覧を機械可読で (名前 TAB パス TAB 出自 TAB 説明)
haj docs [<トピック>]            ドキュメントを読む
haj config [--init]            設定の実効値と出所 (--init で雛形)
haj completion <シェル>          補完スクリプトを出す
haj exec <プログラム> [引数...]   PATH のコマンドを実行 (シークレット注入つき)
haj sh '<コマンド>'              シェルの1行を実行 (同上)
haj selfupgrade                haj自身を更新する
haj --version
```

グローバルフラグは**コマンド名の前**に書く(git 方式。名前より後ろはサブコマンドに素通し)。

```
-C <ディレクトリ>                 そのディレクトリを起点に実行する (git と同じ)
--secret <名前>=<値>             シークレット参照を展開して環境変数で渡す
--env <ファイル>                  key = value を読み、値を展開して渡す
--secretfile <出力>=<テンプレート>  テンプレートを描画して 0600 で書いてから実行
```

組み込みコマンドは**どこにいても使える**ので、一覧にも TAB 補完にも常に出ます
(探索されるコマンドとは節を分けて表示)。どこにいても打てるものが一覧に出ないのは、
「一覧が実態と一致する」という haj の売りに反するからです。

## コマンドを追加する

コマンドの足し方は3通りある。**どれも登録は要らない**(置けば生える)。

| 方式 | 置き場所 | 効く範囲 | 向いているもの |
|---|---|---|---|
| A. プロジェクト | `<リポジトリ>/.haj/commands/<名前>` | そのリポジトリの中だけ | チームで共有する、リポジトリ固有のタスク |
| B. グローバル | `$PATH` の `haj-<名前>` | どこでも | 個人ツール、パッケージマネージャで配るもの |
| C. エイリアス | `~/.config/haj/config` の `alias.<名前>` | どこでも | 打鍵の短縮。既存コマンドの組み合わせ |

### A. プロジェクトのコマンド(`.haj/commands/`)

**基本形。** リポジトリにコミットすれば、チーム全員の `haj` にそのコマンドが生える。

```sh
mkdir -p .haj/commands
cat > .haj/commands/deploy <<'EOF'
#!/bin/bash
set -euo pipefail

# 規約フック。本体より先に処理する(後述)
case "${1:-}" in
  --haj-describe) echo "本番へデプロイする"; exit 0 ;;
  --haj-help)     echo "haj deploy <staging|production>"; exit 0 ;;
  --haj-complete) shift; [ $# -eq 0 ] && printf '%s\n' staging production; exit 0 ;;
esac

echo "==> ${HAJ_PROJECT}: ${1:?環境を指定してください} へデプロイします"
EOF
chmod +x .haj/commands/deploy
```

```console
$ haj                       # 一覧に説明が出る
   deploy     本番へデプロイする   [example-app]
$ haj deploy <TAB>          # staging / production が補完される
$ haj deploy staging
==> example-app: staging へデプロイします
```

**ヘルプにも補完にも1行も書き足していない。** コアがコマンド自身に聞いているから。

### B. グローバルなコマンド(`$PATH` の `haj-<名前>`)

`$PATH` に `haj-<名前>` という実行可能ファイルを置くと、どのディレクトリでも
`haj <名前>` で呼べる(git が `git-foo` を `git foo` にするのと同じ)。

```sh
cat > ~/bin/haj-scratch <<'EOF'
#!/bin/sh
case "${1:-}" in
  --haj-describe) echo "作業用の一時ディレクトリを作って cd する"; exit 0 ;;
esac
mktemp -d /tmp/scratch.XXXXXX
EOF
chmod +x ~/bin/haj-scratch
```

```console
$ haj scratch
/tmp/scratch.a1B2c3
```

- 探索の**最後**なので、プロジェクトの同名コマンドには負ける(意図どおり)
- **`HAJ_ROOT` は渡されない**(属するツリーが無い)。共通ライブラリに依存せず自己完結で書く
- 規約フックは同じように効く。実装すれば一覧にも補完にも出る
- 個人用に置くだけなら `~/.config/haj/commands/<名前>` でもよい(こちらは `haj-` 接頭辞が不要)

### C. エイリアス(`alias.<名前>`)

**新しい実行ファイルは作らず、語の並びに展開する**(git の alias と同じ)。

```sh
haj config --init > ~/.config/haj/config   # まだ無ければ雛形を出す
```

```
# ~/.config/haj/config
alias.web = -C ~/repos/webapp
alias.wm  = -C ~/repos/webapp mig
```

```console
$ haj web help          # → haj -C ~/repos/webapp help   (そのプロジェクトの一覧)
$ haj web deploy prod   # → haj -C ~/repos/webapp deploy prod
$ haj wm up             # → haj -C ~/repos/webapp mig up
```

- 展開は**1回だけ**(再帰しない)。残りの引数は後ろに繋がる
- 優先順位は **組み込み > エイリアス > 探索**(`alias.help` のような予約語は無視される)
- 定義を読むのは**ユーザー設定だけ**。リポジトリからは定義できない
  (clone したリポジトリに `alias.mig = sh '...'` を仕込ませないため)
- `haj which <名前>` で展開を確認でき、`haj` の一覧にもエイリアスの節が出る

### 規約(A / B に共通)

コアはコマンドの中身を知らない。知りたいことは**コマンド自身に聞く**。

| 引数 | 返すもの | |
|---|---|---|
| `--haj-describe` | 一行説明 | 必須。`haj` の一覧に使う |
| `--haj-help` | 詳しい使い方 | 任意。`haj help <名前>` |
| `--haj-complete <入力済みの語...>` | 補完候補(改行区切り) | 任意。TAB補完 |

コアが渡す環境変数:

| 変数 | 意味 |
|---|---|
| `HAJ_ROOT` | そのコマンドが属するツリー。共通ライブラリは `. "$HAJ_ROOT/lib/common.sh"` |
| `HAJ_NAME` | 呼ばれた名前 |
| `HAJ_PROJECT` / `HAJ_PROJECT_DIR` | いま操作対象のプロジェクト。**破壊的なコマンドは対象を名乗ること** |

**規約フックは共通ライブラリを読む前に処理すること。** 説明文を1行返すためだけに
重い初期化をすると、TAB のたびにその分だけ待たされる(フックは2秒で打ち切られる)。

より詳しくは `haj docs writing-commands`(端末で読める)。契約の全文は [SPEC.md](SPEC.md)。

## 探索順

先に見つかったものが勝つ。

| 順 | 場所 | 用途 |
|---|---|---|
| 1 | カレントから上へ辿った `.haj/commands/<名前>` | プロジェクト固有 |
| 2 | `~/.config/haj/commands/<名前>` | 個人用 |
| 3 | `$HAJ_COMMAND_PATH`(既定 `/usr/local/lib/haj/commands`) | 全社/イメージ共通 |
| 4 | `$PATH` の `haj-<名前>` | git 方式の逃げ道 |

## `.haj` は壁である

**1 の遡上は `/` までは行かない。`.haj` を持つディレクトリで止まる。**

止めないと、誰かが `~/repos/.haj/commands/setup` を置いただけで、その配下の**全リポジトリ**に
`haj setup` が生えてしまう。置いた本人以外は気づけない。`setup` や `reset` は破壊的なので、
これは事故になる。

境界と名前は `.haj/project` で宣言する(**無くてもよい**。既定で「境界」かつ「名前は
ディレクトリ名」)。

```
name = example-app
root = true      # 既定。false にすると親の .haj も探しに行く(モノレポ用)
```

継承は常にオプトインなので、**知らないうちに上流のコマンドが効くことはない**。

## どのプロジェクトの setup が走るのか

同じ `haj setup` がプロジェクトごとに違う挙動をする以上、いまどれが効いているのかが
見えないこと自体が欠陥です。3つの方法で常に見えるようにしてあります。

```console
$ haj
 プロジェクト: webapp  (~/repos/example-app/web/webapp)

 hajコマンド (haj help <名前> で詳細):
   bao-login  Vaultにログイン           [共通]
   mig        DBマイグレーション          [example-app]
   setup      webapp のセットアップ   [webapp]

$ haj which --all setup
* ~/repos/example-app/web/webapp/.haj/commands/setup  [webapp]
  ~/repos/example-app/.haj/commands/setup                 [example-app]
  /usr/local/lib/haj/commands/setup                              [共通]

(* が実行されるもの。他は隠れている)
```

さらにコアは **`HAJ_PROJECT` / `HAJ_PROJECT_DIR`** を渡すので、破壊的なコマンドは
対象を名乗れます。

```sh
echo "==> ${HAJ_PROJECT}: セットアップします"
```

`HAJ_ROOT`(そのコマンドがどこから来たか)と `HAJ_PROJECT`(いまどこに対して実行して
いるか)は**別物**です。共通の `mig` をプロジェクトの中で叩けば前者は `/usr/local/lib/haj`、
後者は `example-app` になります。

## 設定

**git と同じ形**です。リポジトリ側は `.haj/`(git の `.git/`)、ユーザー側は
`~/.config/haj/`(git の `~/.config/git/config`)。

| 何 | 場所 |
|---|---|
| ユーザー設定 | `~/.config/haj/config` |
| 個人用コマンド | `~/.config/haj/commands/` |
| プロジェクト設定 | `<リポジトリ>/.haj/project` |
| キャッシュ | `~/.cache/haj/` |

形式は `.haj/project` と**同じ** `key = value`(`#` から行末はコメント)。
設定ファイルの形式が2つあると「どっちがどっちだったか」を覚える羽目になるので、
1つに揃えています。

```
# ~/.config/haj/config
# 鍵は git と同じドット記法(selfupgrade.* / secrets.* / ドット無しはコア)
secrets.vault_cmd  = bao                        # OpenBao を使うなら
secrets.vault_addr = https://vault.example.com
secrets.vault_login = -method=oidc              # 未ログイン時に自動ログイン

hook_timeout_ms = 2000
```

雛形は `haj config --init > ~/.config/haj/config` で出せます。

値は **環境変数 > 設定ファイル > 既定値** の順で決まります。この3段が見えないと
「なぜ効かないのか」を調べる手段が無くなるので、`haj config` が**実効値と一緒に
出所を出します**(`haj which` が探索順を見せるのと同じ理由)。

```console
$ haj config
設定ファイル: /home/kurari/.config/haj/config

  command_path            /usr/local/lib/haj/commands   (既定値)
  hook_timeout_ms         5000                          (設定ファイル)

  secrets.vault_cmd       bao                           (設定ファイル)
  secrets.vault_addr      https://vault.example.com     (設定ファイル)

  selfupgrade.github      AvapCoLtd/haj                 (既定値)
  selfupgrade.token       (未設定)
```

`selfupgrade.token` は値を出しません(シェルの履歴やスクリーンショットに残るため)。
設定されているかと、どこから来たかだけを示します。ただし**シークレット参照**なら
参照そのものを出します(参照は秘密ではないし、どこの金庫を指しているかは調べたい情報)。

### シークレット

値の実体ではなく**参照**を書ける。haj が exec の直前に解決して渡す。

```sh
haj --secret DB_PASS=vault://secret/data/db/password mig up
haj --secret TOKEN=op://Infra/ci/token exec sh -c 'curl -H "Authorization: Bearer $TOKEN" ...'
```

書式は発明していない — 1Password は `op inject`、Vault は vault-agent template の
展開式をそのまま受ける(`vault://<パス>/<フィールド>` の短縮形も可)。解決に失敗したら
**コマンドは実行されない**(fail-fast)。詳しくは `haj docs spec` の §10。

## ディレクトリ構成(ツリー)

```
<ツリー>/
  commands/          ← 実行可能ファイルを置く。ここにある名前がコマンドになる
    mig
    deploy
  lib/               ← 共通ライブラリ。$HAJ_ROOT/lib/... で読める
    common.sh
  help.header        ← haj help の先頭に出す固定文(任意)
  help.footer        ← haj help の末尾に出す固定文(任意)
```

`haj help` は **header + 自動生成のコマンド一覧 + footer** を出す。
コマンド一覧を手で書かないこと。

例は [examples/](examples/) にある。

## ドキュメント

| どこで | 何が |
|---|---|
| `haj docs writing-commands` | コマンドの作り方(端末で読める) |
| `haj docs spec` | コアとサブコマンドの契約の全文 |
| [SPEC.md](SPEC.md) | 同上(リポジトリ版) |
| [examples/](examples/) | 規約フックまで実装した実物 |
| [CONTRIBUTING.md](CONTRIBUTING.md) | 開発・テスト・リリースの手順 |

## ライセンス

MIT
