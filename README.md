# haj

**h**ack **a**pplication **j**ob — プロジェクトごとに中身が変わるジョブランナー。

`haj` はサブコマンドを**持たない**。そこに置いてある実行可能ファイルを**探して**実行する。
だから、リポジトリごとに使えるコマンドが違う、という状態が自然に成立する。

```console
$ cd ~/repos/webapp && haj
 hajコマンド (haj help <名前> で詳細):
   web      基本版(webapp)の操作
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

### バイナリ (Linux x86_64)

依存ゼロの静的バイナリ。glibc も bash も要らない(alpine でも動く)。

```sh
VERSION=0.1.0
TOKEN=<あなたのGitLabトークン>   # このリポジトリはprivateなので必要
curl -fsSL --header "PRIVATE-TOKEN: $TOKEN" \
  "https://gitlab.avaper.day/api/v4/projects/788/packages/generic/haj/${VERSION}/haj-x86_64-unknown-linux-musl.tar.gz" \
  | tar xz
sudo install -m 755 haj /usr/local/bin/haj
```

`install.sh` を使うと上記を自動でやる。

```sh
HAJ_TOKEN=<トークン> ./install.sh            # 最新版
HAJ_TOKEN=<トークン> ./install.sh 0.1.0      # 版を指定
```

### 他のプラットフォーム

CI は Linux x86_64 のみをビルドする。それ以外は手元でビルドしてほしい。

```sh
cargo build --release
install -m 755 target/release/haj /usr/local/bin/haj
```

Rust さえ入っていれば macOS でも Arm でもそのまま通る(依存クレートはゼロ)。

## 使い方

```
haj <コマンド> [引数...]     コマンドを実行する
haj                        コマンド一覧
haj help <名前>             そのコマンドの詳しい使い方
haj which <名前>            探索で勝っている実行ファイルのパス
haj commands               一覧を機械可読で (名前 TAB パス TAB 説明)
haj --version
```

## コマンドを追加する

**実行可能ファイルを置くだけ。** 登録も設定ファイルも要らない。

```sh
mkdir -p .haj/commands
cat > .haj/commands/deploy <<'EOF'
#!/bin/bash
set -euo pipefail

case "${1:-}" in
  --haj-describe) echo "本番へデプロイする"; exit 0 ;;
  --haj-help)     echo "haj deploy <staging|production>"; exit 0 ;;
  --haj-complete) shift; [ $# -eq 0 ] && printf '%s\n' staging production; exit 0 ;;
esac

echo "deploying to ${1:?環境を指定してください}..."
EOF
chmod +x .haj/commands/deploy
```

これで `haj deploy` が使え、`haj` の一覧に説明が出て、`haj deploy <TAB>` が
`staging` / `production` を補完する。**ヘルプにも補完にも1行も書き足していない。**

詳しい契約は [SPEC.md](SPEC.md) を参照。要点だけ:

| 引数 | 返すもの | |
|---|---|---|
| `--haj-describe` | 一行説明 | 必須。`haj` の一覧に使う |
| `--haj-help` | 詳しい使い方 | 任意。`haj help <名前>` |
| `--haj-complete <入力済みの語...>` | 補完候補(改行区切り) | 任意。TAB補完 |

コアは `HAJ_ROOT`(そのコマンドが属するツリー)と `HAJ_NAME` を環境変数で渡すので、
共通ライブラリは `. "$HAJ_ROOT/lib/common.sh"` で読める。

**規約フックは共通ライブラリを読む前に処理すること。** 説明文を1行返すためだけに
重い初期化をすると、TAB のたびにその分だけ待たされる。

## 探索順

先に見つかったものが勝つ。

| 順 | 場所 | 用途 |
|---|---|---|
| 1 | カレントから上へ辿った `.haj/commands/<名前>` | プロジェクト固有 |
| 2 | `~/.haj/commands/<名前>` | 個人用 |
| 3 | `$HAJ_COMMAND_PATH`(既定 `/usr/local/lib/haj/commands`) | 全社/イメージ共通 |
| 4 | `$PATH` の `haj-<名前>` | git 方式の逃げ道 |

どれが勝っているか分からなくなったら `haj which <名前>`。

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

## シェル補完

```sh
# zsh
install -m 644 completions/_haj /usr/local/share/zsh/site-functions/_haj

# bash
install -m 644 completions/haj.bash /etc/bash_completion.d/haj
```

補完スクリプトは候補を一切持たない。`haj __complete` に聞くだけなので、
**コマンドを足しても更新不要**。

## 開発

依存クレートはゼロ。標準ライブラリだけで書く、というのが設計上の制約
(haj がやるのは探索と exec だけで、CPU の仕事は無い。clap や serde を持ち込んでも
ビルド時間と監査対象が増えるだけで得るものが無い)。

```sh
cargo test                                 # 統合テスト
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

テストは一時ディレクトリに本物の実行ファイルを置いて `haj` を外から叩く。
探索順・同名の優先度・規約・フックのタイムアウト・終了コードの伝播といった、
この道具の本質そのものを検証している。

## リリース

`Cargo.toml` の版を上げ、タグを打つと CI が静的バイナリをビルドして
Package Registry と Release に公開する。

```sh
git tag v0.2.0 && git push origin v0.2.0
```

タグ(`v` を除いた部分)と `Cargo.toml` の `version` が食い違う場合、CI は失敗する。
