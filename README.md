# haj

**h**ack **a**pplication **j**ob — CLIゲートウェイ兼 JOBランナー

## hajコマンドは何か

haj は、エンジニアの CLI 操作を**実行可能な知識**として封じ込めるためのゲートウェイである。
手順書は書いた瞬間から実態と乖離していくが、コマンドは日々実行される限り乖離できない —
動かなくなった知識はその場で発覚し、直される。プロジェクトの手順・個人の定型・チームの
運用知識を「置けば生える」コマンドやタスクにすることで、備忘録を実態と一致したまま
共有可能にする。

もうひとつの柱は機密情報の統一管理である。各ツールに資格情報を個別に持たせると、
平文のトークンや鍵がローカルディスクに散らばり、棚卸しも失効もできなくなる。
haj はすべての機密情報を Vault への**参照**として扱う統一の口を提供し、値は実行の瞬間に
引いて子プロセスへ渡し、揮発させる — ローカルに常駐する平文を作らないことを、
個人の注意ではなく仕組みで保証する。

### 仕組み

**1. 統一した規約フック。** コマンドはただの実行可能ファイルだが、規約のオプションに
応答することで、説明・ヘルプ・補完・環境変数が haj から一律に引けることを保証する:

```sh
#!/bin/sh
case "${1:-}" in
  --haj-describe) echo "本番へデプロイする"; exit 0 ;;            # haj help の一覧に出る
  --haj-complete) shift; printf '%s\n' staging production; exit 0 ;;  # TAB 補完
  --haj-env)      echo "DEPLOY_TARGET=${DEPLOY_TARGET:-staging}"; exit 0 ;;  # haj env で見える
esac
echo "deploy to ${1:?}"   # ここから本体
```

**2. Vaultとのシームレスな連動。** 値の実体ではなく参照(`vault://` / `op://` など)を
書き、解決は haj がやる。呼び出されるコマンドは環境変数を読むだけで、Vault の存在を
知らなくてよい:

```
# ~/.config/haj/config — 宣言(値はここに書かない)
user.secret.DB_PASS = vault://secret/data/db/password
```

```sh
haj --secret DB_PASS=vault://secret/data/db/password mig up   # その実行だけ注入
db_pass=$(haj secret get DB_PASS)                             # 宣言を要る瞬間に引く (pull)
```

**3. 自動揮発の保証。** 受け渡しは環境変数か、`$XDG_RUNTIME_DIR` 配下の一時ファイル
(0600)に限る。ファイルはセッション終了で OS が消す — 掃除の手順も、消し忘れも無い:

```sh
keyfile=$(haj secret file OCI_KEY)      # 0600 で実体化してパスを返す。cwd には書かない
haj --secret-file KEY=vault://secret/data/ssh/id_rsa sh 'ssh -i "$KEY" host'
```

## 活用方法

### 自分のPCのCLIを安全で便利にする

日常使いの CLI(`oci` / `glab` / `aws` など)は資格情報をホームの設定ファイルに平文で
置かせたがる。haj でラップすれば、**手元には参照だけ**を置いて普段どおりに使える:

```
# ~/.config/haj/config
alias.oci = --secret OCI_CLI_USER=vault://users/me/oci/user \
            --secret-file OCI_CLI_KEY_FILE=vault://users/me/oci/private_key \
            exec oci
```

`haj oci iam region list` — 鍵は実行の瞬間だけ 0600 の一時ファイルに実体化され、
セッション終了で消える。`~/.oci/config` はもう要らない。シェルの `alias oci='haj oci'` や
PATH のシムを重ねれば素のコマンド名のまま使え、ディスクに平文が無いので、
ラップトップの紛失やマルウェアの持ち出しに対して被害が資格情報に及ばない。

### プロジェクト環境に専用コマンドを作る

`.haj/commands/` はリポジトリにコミットして配れる。コマンドは秘密を持たず**要る瞬間に
引く**ので、clone された先がどこでも — 共有の踏み台、リモートの作業サーバ、コンテナ —
同じものが安全に動く:

```sh
#!/bin/sh
# <repo>/.haj/commands/mig — チーム全員が使うマイグレーション
db_pass=$(haj secret get DB_PASS) || exit 1   # 実行者自身の Vault 認証で解決される
...
```

サーバに共有のトークンや `.env` を置かないので、退職や漏洩のたびに配り直す運用が消える。
資格情報は各実行者の Vault 認証に紐づくため、「誰の権限で動いたか」も Vault の監査ログに
そのまま残る。

## インストール方法

Linux x86_64 / aarch64 の静的バイナリ(musl)。glibc も bash も要らない:

```sh
curl -fsSL https://raw.githubusercontent.com/AvapCoLtd/haj/master/install.sh | sh
```

既定で `/usr/local/bin/haj` に入る(`HAJ_PREFIX=$HOME` で `~/bin` へ)。インストーラは
アーキテクチャを自動判別し、`.sha256` で改竄と転送事故を検出する。他のプラットフォームは
`cargo build --release` — 依存クレートはゼロなので Rust さえあれば通る。

シェル補完は1行:

```sh
eval "$(haj completion zsh)"   # ~/.zshrc(bash も同様)
```

### アップグレード方法

```sh
haj selfupgrade            # 最新版に入れ替える(最新なら何もしない)
haj selfupgrade --check    # 調べるだけ (0=最新 / 1=更新あり / 2=調べられず)
haj selfupgrade 0.39.0     # 版を指定(ダウングレードもこれ)
```

認証は要らない。置き換えは同じディレクトリに書いてから `rename(2)` するので原子的で、
実行中のプロセスは壊れない。

## チュートリアル

`haj-credless` ツリー(公開予定)を入れて、`oci` を資格情報レスで動かすまで。

**1. Vault への接続を設定する**(`~/.config/haj/config`):

```
secrets.vault_cmd   = bao
secrets.vault_addr  = https://vault.example.com
secrets.vault_login = -method=oidc
```

未ログインなら参照の解決時に自動でログインが走るので、事前の `vault login` は要らない。

**2. ツリーを入れて、初期値を取り込む:**

```console
$ haj tree install https://github.com/AvapCoLtd/haj-credless --name credless
$ haj tree configure credless    # 提案された tree.credless.* を確認して y/N で取り込む
$ haj secret check --tree credless   # 宣言と受け渡しを検証(Vault には触らない)
```

**3. 動かす:**

```console
$ haj oci iam region list
```

初回は OIDC ログイン(ブラウザ)が挟まり、以降はセッションが続く限りそのまま動く。
鍵は実行の瞬間だけ 0600 の一時ファイルに実体化され、セッション終了で消える —
`~/.oci/` に置いていた設定と鍵は、もう削除してよい。仕組みの全景は
`haj config --tree credless` で1コマンドで見える。

## ゲートウェイ機能

`haj` は1つの入口だが、立っている場所によって語彙が変わる。どこでも同じに使える
組み込みサブコマンド(`help` / `config` / `secret` / `tree` など)を土台に、
置き場所ごとの定義が重なる:

```
<リポジトリ>/.haj/commands/    プロジェクト固有(そのリポジトリの中でだけ生える)
~/.config/haj/commands/        個人用(どこでも)
~/.local/share/haj/trees/      ツリー(git で配布されるコマンド群)
/usr/local/lib/haj/commands/   全社・イメージ共通
```

同名は**プロジェクト > 個人 > ツリー > 共通**の先勝ち。探索は cwd から上へ遡り、
最初の `.haj` で止まる — リポジトリに入った瞬間、コマンドセットがそのプロジェクトの
ものに切り替わる。どの定義が効いているかは `haj which --all <名前>` で常に見える。

コマンドを書くほどでもない1行は `.haj/config` のエイリアスで、install / test のような
「このリポジトリの作業」は探索に乗らない**タスク**として `haj run` に隔離する:

```
# .haj/config
alias.logs = exec docker compose logs -f app
task.test  = exec docker compose exec app vendor/bin/phpunit
```

```console
$ haj logs        # このプロジェクトでだけ通じる語彙
$ haj run test    # タスクは haj run 経由のみ。他プロジェクトのコマンドと紛れない
```

→ コマンドの書き方と規約フックの詳細: [コマンドの作り方](docs/writing-commands.md)(`haj docs writing-commands`)

**未解決課題: ホームディレクトリ専用のコマンド。** いまの置き場は「このリポジトリだけ」
(プロジェクト)と「どこでも」(個人)の2粒度しかなく、その中間 — dotfiles の同期や
ホームの掃除のような「$HOME というプロジェクトの作業」の置き場がない。個人用に置くと
すべてのプロジェクトの語彙に混ざり、補完のノイズや破壊的コマンドの事故の種になる。

`~/.haj/` を置いてホーム自体をプロジェクトにする回避策はあるが、副作用が未整理:
`.haj` を持たないホーム配下の全ディレクトリが「ホームプロジェクト」として解決される
ことをどう扱うか、そして `~/.haj/commands`(プロジェクト層)と `~/.config/haj/commands`
(個人層)で個人レイヤーが2枚になり「個人のコマンドはどっちに置くのか」が濁る。
ホームをプロジェクトとして公認するか、専用のスコープ(ホーム外では生えない個人コマンド)
を設けるか、設計判断が下りていない。

## TREE (配布可能なナレッジの塊)

ツリーは、コマンド・エイリアス・ドキュメントをひと塊にした git リポジトリである。
「このドメインの知識一式」を**小分けの単位で配布**できる — 全部入りの社内モノレポでは
なく、必要なツリーだけを選んで入れる。エイリアス集だけのツリーも、ドキュメントを
同梱したツリーも正当で、入れた瞬間から探索・`haj help`・`haj docs` に出自付きで載る。

パッケージマネージャではない。clone したディレクトリが探索の対象になるだけで、
**git のリポジトリ自体が状態**(URL は remote、版は HEAD)。ビルドもロックファイルも無い。
使う側は動詞4つ:

```sh
haj tree install https://github.com/you/haj-tools@v1   # 入れる(@<ref> で固定可)
haj tree update                                        # 差分 (git log) を見せてから ff-only で更新
haj tree configure haj-tools                           # ツリーの初期値提案を確認してユーザー設定へ
haj tree remove haj-tools                              # 消す
```

install は URL を自分で打つ行為 = 信頼の表明なので、知らないツリーを入れるときは
中身を読むこと。

→ ツリーの作り方(リポジトリの形、config-init、quickref、コアがツリーに提供する
機能の全景): [ツリーの作り方と配布](docs/trees.md)(`haj docs trees`)

### 設定のオーバーライド

ツリーのコマンドは接続先やアカウントをハードコードせず、`VAR="${VAR:-既定値}"` の形で
環境変数に昇格して受ける。恒常的な上書きは**ユーザー設定**の `tree.<インストール名>.env.*`:

```
# ~/.config/haj/config
tree.work.env.MYAPP_ACCOUNT    = alice@example.com   # 平文をそのまま注入(展開しない)
tree.work.secret.CLIENT_SECRET = vault://secret/data/myapp/client_secret  # 秘密は宣言 — 注入されない
```

その場限りの上書きは、実行時のシェル環境かフラグで:

```sh
MYAPP_ACCOUNT=bob@example.com haj work sync          # この実行だけ差し替え
haj --secret CLIENT_SECRET=vault://.../alt work sync # 秘密もフラグが勝つ
```

優先順位は**フラグ > シェル環境 > tree 設定 > コマンドの既定値**で、未設定の変数にだけ
注入される。実効値と出所は `haj env <ツリー名> <コマンド>` が行末コメントで注記する。
権威はユーザー設定だけ — ツリー自身のリポジトリに書かれた `tree.*` は無視される。
配布された設定が勝手に効くことは無く、`haj tree configure` の提案を**確認して取り込む**
ことが同意になる。

## リファレンス

この文書は入口まで。それぞれの全景は個別のリファレンスで:

| 知りたいこと | 参照先 |
|---|---|
| 組み込みコマンドの一覧と使い方 | `haj help`(生きた一覧)/ [組み込みコマンド](docs/builtins.md) |
| 設定できる鍵の全て | `haj config --init`(全鍵の雛形)/ [設定リファレンス](docs/config.md) |
| 秘密の参照書式・宣言・store | [シークレット](docs/secrets.md)(`haj docs secrets`) |
| コマンドの作り方(規約フック) | [コマンドの作り方](docs/writing-commands.md)(`haj docs writing-commands`) |
| ツリーの作り方・配布・提供される機能 | [ツリーの作り方と配布](docs/trees.md)(`haj docs trees`) |
| いま何ができるかを一枚で | `haj help --quick`(コアと全ツリーの圧縮リファレンス) |
| コアとコマンドの契約の全文 | [SPEC](SPEC.md)(`haj docs spec`) |

## 開発情報

- https://github.com/AvapCoLtd/haj (公開用)
- https://gitlab.avaper.day/avap/haj/haj (開発用)