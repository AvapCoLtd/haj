# 組み込みコマンド(コアの動詞)

コアが持つ動詞の一覧と使い方。これらは**予約語**で、同名のコマンドを置いても奪えない
(`exec` や `sh` を奪えるとシークレットの横取りができてしまうため)。どのプロジェクトに
居ても常に同じに使える。人間向けの生きた一覧は `haj help`、この文書は個別の早引き。

## 見る・調べる

### haj help

```
haj help              いま使えるコマンドの一覧 (組み込み + 探索で生えたもの、出自つき)
haj help <名前>       個別の使い方 (コマンド自身の --haj-help に聞く)
haj help --quick      コアと全ツリーの圧縮リファレンスを一枚で (AI セッションに渡す出口)
```

### haj commands

コマンド一覧を機械可読で出す(`名前 <TAB> パス <TAB> 出自 <TAB> 一行説明`)。
スクリプトから列挙するために使う。

### haj which

```
haj which <名前>          実行される実行ファイルのパスを出す
haj which --all <名前>    同名の候補を探索順に全部出す (* が実行されるもの)
```

探索は cwd に依存する。`setup` や `reset` のような破壊的コマンドは、迷ったら
実行前にこれで確認する。

### haj env

```
haj env <名前>               そのコマンドが読む環境変数を KEY=value で出す
haj env run <タスク>          タスク版
haj env <ツリー名> [<名前>]   ツリーのコマンド版 (名前を省くと全コマンド分を連結)
```

出力はそのまま `--env-file` に渡せる: `haj env mig > env.txt` → 編集 →
`haj --env-file env.txt mig`。中身はコマンド自身の `--haj-env` に聞くだけ。

### haj config

```
haj config                        実効値と出所の一覧
haj config --init                 設定できる鍵と既定値をすべて雛形として出す (全行コメント)
haj config get <キー>              実効値を1行で (plumbing。未設定は exit 1)
haj config set <キー> <値>         ユーザー設定へ書く (既存キーは行を置換)
haj config --tree <インストール名>  そのインスタンスの全景 (tree.*、store、実効 env)
```

鍵の意味は [設定リファレンス](config.md)。

### haj docs

```
haj docs             トピック一覧 (出自つき。fzf があれば選択 UI)
haj docs <トピック>   その文書を素の markdown で出す (長いものはページャへ)
```

ツリーは `docs/` を同梱でき、コマンドと同じ探索で生える。

## 実行する

### haj run

```
haj run                このプロジェクトのタスク一覧
haj run <名前> [引数]   タスクを実行 (以降の引数はそのまま渡る)
```

タスクは現在のプロジェクトの `task.<名前>`(`.haj/config`)と `.haj/tasks/<名前>` だけを
見る。遡らない・上書きしない・フォールバックしない。

### haj exec

PATH のコマンドにシークレットを注入して実行する(`op run` / `doppler run` の位置)。
探索は通さない。「注入は欲しいが haj のコマンドにするほどではない」一回きり用:

```sh
haj --secret DB_HOST=vault://secret/data/db/host exec sh -c 'mysql -h $DB_HOST'
```

haj は文字列をシェルに包まない — `$VAR` の展開が要るなら明示的に `sh -c` を書くか
`haj sh` を使う。

### haj sh

`haj exec sh -c '<コマンド>'` の省略形。追加の引数は位置パラメータになる:

```sh
haj --secret MYSQL_HOST=vault://secret/data/db/host sh 'mysql -h $MYSQL_HOST'
haj sh 'echo $1-$2' one two    # → one-two
haj sh -- ls -la               # '--' 以降を空白で繋いで1行に (ssh 方式)
```

## 秘密

### haj secret

宣言された秘密を引く。読みだけ。宣言域は文脈で決まる — ツリーのコマンドの中は
`tree.<インストール名>.secret.*`、外(シェル・個人コマンド)は `user.secret.*`:

```
haj secret get <KEY>                       宣言を解決して値を stdout へ
haj secret file <KEY>                      0600 のファイルに実体化してパスを stdout へ
haj secret template <KEY> [--out <パス>]   テンプレート宣言を描画して実体化
haj secret tmpdir <名前>                   セッション寿命の管理ディレクトリ (0700) を確保
haj secret list [--tree <名前>]            宣言の一覧
haj secret check [--tree <名前>]           宣言と受け渡しの検証 (Vault には触らない)
```

宣言に無い KEY はエラー。詳細は [シークレット](secrets.md)。

### haj store

自ツリー専用のストア(`<prefix>/trees/<HAJ_TREE>/`)を読み書きする。
実行時に得たトークン等の置き場:

```
haj store get <論理パス>            値を stdout へ
haj store put [--force] <論理パス>  stdin から読んで書く (フィールド単位の patch)
haj store login                    エンジンにログインする
haj store status                   ログイン状態と実効設定
```

put は stdin 限定(argv は `ps` に見える)。シェルから直に叩くときはツリー文脈が
無いのでエラーになる — 点検は物理参照で(`haj store get vault://<物理パス>`)。

## 配布・更新

### haj tree

```
haj tree install <URL>[@<ref>] [--name <名前>] [--global]
haj tree update [<名前>]       差分 (git log) を見せてから ff-only で更新
haj tree list                  名前 / 版 / コマンド数 / URL
haj tree configure <名前>      ツリーの初期値提案を確認してユーザー設定へ追記
haj tree remove <名前>
```

詳細は [ツリーの作り方と配布](trees.md)。

### haj selfupgrade

```
haj selfupgrade            最新版に入れ替える (最新なら何もしない)
haj selfupgrade --check    調べるだけ (0=最新 / 1=更新あり / 2=調べられず)
haj selfupgrade <版>       版を指定 (再インストール / ダウングレードもこれ)
```

置き換えは同じディレクトリに書いてから rename するので原子的。private な取得元は
`selfupgrade.gitlab` / `project_id` / `token` を設定([設定リファレンス](config.md))。

### haj completion

```
eval "$(haj completion zsh)"    # ~/.zshrc (bash 版もある)
```

候補はスクリプトが持たず、コアの `haj __complete` に聞くだけ。コマンドを足しても
補完は自動で追従する。

## グローバルフラグ(コマンド名の前に書く)

```
-C <ディレクトリ>                               そのディレクトリを起点に実行 (git と同じ。複数可)
--secret <名前>=<参照>                          参照を展開して環境変数で渡す
--env-file <ファイル>                           key = value を読み、値を展開して渡す
--secret-file <名前|パス>=<参照|テンプレート>   中身をファイルにして渡す (0600)
```

コマンド名より後ろは無解釈で素通し — サブコマンド自身の引数と衝突しない。
参照の書式は [シークレット](secrets.md)、契約の全文は `haj docs spec`。