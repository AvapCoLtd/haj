# haj

**h**ack **a**pplication **j**ob — AVAP社のCLIゲートウェイ兼 JOBランナー

`haj` はプロジェクト/個人/共有の様々なコンテキストで実行できる内容、ドキュメントが変化する。統一的な機密情報へのプロキシを行い、取得した機密情報は揮発させ、ローカルディスクに保持しない。プロジェクト知識やLinux知識を実行可能なJob化することで実態と乖離しない備忘録や共有可能な知識にするものある。

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

### シェル補完の有効化

```sh
# ~/.zshrc
eval "$(haj completion zsh)"

# ~/.bashrc
eval "$(haj completion bash)"
```

補完スクリプトは候補を一切持たない。`haj __complete` でコアに聞くだけなので、
**コマンドを足しても更新は要らない**(プロジェクト固有のコマンドもそのまま候補に出る)。

### 更新方法

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

## コマンドの実行方法

```
haj <グローバルフラグ> <サブコマンド> <引数...
```

### 組込済サブコマンド

```
help            使い方を表示する (haj help <名前> で個別)
commands        コマンド一覧を機械可読で出す
which           どの定義が効いているかを見る (--all で隠れているものも)
config          設定の実効値と出所を見る (--init で雛形を出す)
selfupgrade     haj自身を更新する
secrets         何が渡るのかを解決せずに確かめる (--check)
exec            PATHのコマンドにシークレットを注入して実行する
docs            ドキュメントを読む (コマンドの作り方・仕様・ツリーの文書)
completion      シェル補完のスクリプトを出す (eval "$(haj completion zsh)")
sh              シェルの1行をシークレットを注入して実行する (exec sh -c の省略形)
tree            共有ツリーの取得と更新 (install/update/list/remove)
```

### グローバルフラグ

```
-C <ディレクトリ>                               そのディレクトリを起点に実行する (git と同じ)
--secret <名前>=<値>                            参照を展開して環境変数で渡す
--env-file <ファイル>                           key = value を読み、値を展開して渡す
--secret-file <名前|パス>=<参照|テンプレート>   値をファイルに書く (名前ならパスを環境変数へ)
```

## 共有ツリー

gitリポジトリで公開されているサブコマンドやドキュメント、エイリアスを利用できる

```sh
haj tree install https://github.com/you/haj-tools    # 入れる(@<ref> で固定可)
haj tree update                                      # 差分を見せてから更新
haj tree list                                        # 導入済一覧
haj tree remove haj-tools                            # 導入したツリーの削除
```

## コマンドのワークフロー

(例) 機密情報をbao/vaultに補完している場合

1. 設定を行う (~/.config/haj/config)

機密情報を扱いたい場合接続先のvaultパスを追加する。
vaultでなくコンソールの1passwordなどであれば不要

```
secrets.vault_cmd = bao
secrets.vault_addr = https://(baoのアドレス)
secrets.vault_login = -method=oidc -path=(baoの認証パス)
```

2. 個人用エイリアスを作る (~/.config/haj/config)

設定ファイルに記載する。コマンドが複数行になる場合バックスラッシュで改行をエスケープする。
.descを設定すると、その内容がhelpにでる

```
alias.hello = echo hi hoge
alias.hello.desc = テスト用
```

```console
$ haj hello
hi hoge
```

3. エイリアスで書くのが辛くなってきたらコマンドを作る

~/.config/haj/commands に規定のオプションで応答するようにコマンドを書く

```
cat > ~/.config/haj/commands/deploy <<EOF
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
chmod +x ~/.config/haj/commands/deploy
```

```console
$ haj deploy staging
==> example-app: stagingへデプロイします
```

4. 同様に、プロジェクトルートに書く場合

<REPO ROOT>/.haj/{config,commands}を対象に同様に行う。


## エイリアス設定例


### 機密情報を残さずにociコマンドを実行

```
alias.oci = --secret OCI_CLI_USER=vault://users/hajime/oci/user \
            --secret OCI_CLI_TENANCY=vault://users/hajime/oci/tenancy \
            --secret OCI_CLI_FINGERPRINT=vault://users/hajime/oci/fingerprint \
            --secret OCI_CLI_REGION=vault://users/hajime/oci/region \
            --secret-file OCI_CLI_KEY_FILE=vault://users/hajime/oci/private_key \
            exec oci
alias.oci.desc = OCI CLI を bao の資格情報で起動する
```

### 機密情報を残さずにglabコマンドを実行

```
alias.glab = --secret-file GLAB_CONFIG_DIR/config.yml=~/.config/glab-cli/config.yml.tpl \
            exec glab
alias.glab.desc = glab を bao の資格情報で起動する
```

### 存在を隠蔽する

素の `glab` / `oci` を打っても haj 経由(=認証情報非保持)になるようにラップする。

自分のシェルだけでよければ zshrc / bashrc に:

```sh
alias glab='haj glab'
alias oci='haj oci'
```

シェルの alias は対話シェルにしか効かない。Lens のように**バイナリを直接実行する
アプリケーション**にも効かせるには、PATH の先頭側にシムを置く:

```sh
cat > ~/bin/glab <<'EOF'
#!/bin/sh
exec haj glab "$@"
EOF
chmod +x ~/bin/glab
```

**このときエイリアスの `exec` は本物の絶対パスにすること**(`haj exec` は PATH を
引くので、`exec glab` のままだとシム自身をまた拾って無限ループする。絶対パスなら
PATH を引かない):

```
alias.glab = --secret-file GLAB_CONFIG_DIR/config.yml=~/.config/glab-cli/config.yml.tpl \
            exec /usr/local/bin/glab
```


## もっと知る

```sh
haj docs    # 使い方ガイドと仕様の全文が端末で読める(fzfがあれば選んで読める)
```

## 開発情報

- https://github.com/AvapCoLtd/haj (公開用)
- https://gitlab.avaper.day/avap/haj/haj (開発用)
