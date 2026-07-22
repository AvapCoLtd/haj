# シークレットの受け渡し(--secret / --env-file / --secret-file)

値の実体ではなく**参照**を書く。haj が exec の直前に解決して子プロセスへ渡すので、
リポジトリにもディスクにも平文が残らない。**渡すものと相手は、人がその実行時に
明示する** — haj が環境変数を勝手に走査して展開することはない。

## まず試す

```sh
# 環境変数で渡す(いちばん基本)
haj --secret DB_PASS=vault://secret/data/db/password mig up

# key = value のファイルから(値ごとに参照/平文が混ざってよい)
haj --env-file ./mig.env mig up
# コマンドが --haj-env に対応していれば、雛形はコマンド自身から出せる:
#   haj env mig > mig.env → 編集 → haj --env-file mig.env mig up

# 「ファイルで渡せ」と要求するツールに(ssh鍵・kubeconfig・SA JSONなど)
haj --secret-file KEY=vault://secret/data/ssh/id_rsa sh 'ssh -i "$KEY" host'

# hajのコマンドにするほどでもない一回きりは exec / sh へ
haj --secret TOKEN=op://Infra/ci/token exec curl -H "Authorization: Bearer $TOKEN" ...
```

フラグは**サブコマンド名の前**にだけ書く(git 方式)。名前より後ろは無解釈で
素通しなので、サブコマンド自身の引数と衝突しない。

## 何が渡るのかを金庫に触らずに確認する

```console
$ haj --secret DB_PASS=vault://secret/data/db/password --env-file ./mig.env secret check
 実行時に渡るもの (値は解決していません):
   --secret    DB_PASS               → vault://secret/data/db/password
   --env-file  DB_HOST                 db.internal
   --env-file  DB_USER               → vault://secret/data/db/user
```

`→` が付いたものが展開される。値は解決しないので、OIDCログインもタッチ認証も
起きない(参照そのものは秘密ではない)。

## 参照の書式(発明していない)

| 参照 | 意味 |
|---|---|
| `vault://<パス>/<フィールド>` | Vault/OpenBao。最後のセグメントがフィールド |
| `store://<論理パス>` | **そのコマンドが属するツリー専用**の名前空間(下記)。最後のセグメントがフィールド |
| `{{ with secret "<パス>" }}{{ .Data.data.<フィールド> }}{{ end }}` | vault-agent template の正準形をそのまま |
| `op://<金庫>/<アイテム>/<フィールド>` | 1Password。`op inject` に丸ごと委譲 |
| `env://VAR` | 環境変数の値(1段だけ) |
| `file://<パス>` | ファイルの中身(docker secrets / systemd credentials との接続に) |

**値全体が参照のときだけ**展開する。文字列中への埋め込みは解釈しない
(接続文字列の組み立てはコマンド側の責務)。解決に失敗したら本体を実行せずに
exit 1(未解決の参照文字列がパスワードとして使われる事故を防ぐ)。

## --secret-file の左辺は3形態

| 左辺 | 動き |
|---|---|
| `KEY`(環境変数として妥当な名前) | 一時ファイルに書き、そのパスを環境変数 `KEY` に入れる |
| `GLAB_CONFIG_DIR/config.yml`(名前/相対パス) | 一時**ディレクトリ**の中に書き、ディレクトリのパスを環境変数に入れる |
| `~/.npmrc` などのパス | そのパスに書く |

右辺が参照ならその値がファイルの中身になり、参照でなければ**テンプレートファイル**の
パスとみなして描画する(vault-agent の `{{ with secret ... }}` テンプレートが
そのまま動く)。一時ファイルは `$XDG_RUNTIME_DIR` 配下に 0600 で作られ、
cwd には決して書かない。

## 金庫の設定(~/.config/haj/config)

```
secrets.vault_cmd   = bao                        # CLIの差し替え(既定 vault)
secrets.vault_addr  = https://vault.example.com  # 環境の VAULT_ADDR / BAO_ADDR が優先
secrets.vault_login = -method=oidc               # 未ログイン時の自動ログイン。off で無効
```

未ログインで `vault://` を解決しようとすると、`secrets.vault_login` の引数で
`login` が**端末を継いで**自動実行される。認証しない CI で参照を使うなら
`HAJ_VAULT_LOGIN=off` を置くこと(OIDC はブラウザと人を待つ)。

## ツリーごとの設定と秘密の宣言(tree.*)

常用するツリーに毎回フラグを打たなくてよい。ユーザー設定に書く。
**env と secret は渡り方が違う** — 平文は環境変数として注入されるが、
**秘密は環境に勝手に載らない**(コマンドが要る瞬間に自分で引く):

```
# ~/.config/haj/config
tree.work.env.MYAPP_ACCOUNT    = alice@example.com     # 平文をそのまま注入(展開しない)
tree.work.env.TOKEN_OUTPUT     = token                 # 論理パスもただのデータ
tree.work.secret.CLIENT_SECRET = vault://secret/data/myapp/client_secret  # 宣言(注入されない)
```

- `.env` の優先順位: **フラグ > シェル環境 > tree設定 > コマンドの既定値**
  (`${VAR:-...}`)。未設定の変数にだけ注入される。出所は
  `haj env <ツリー名> <コマンド>` が行末コメントで注記する
- `.secret` は**宣言**(このツリーが扱える秘密の目録)。コマンドが
  `haj secret get <KEY>` で引いたときだけ解決される。参照でない値(平文)は
  get / check がエラーにする(秘密の平文を設定ファイルに書かせない)
- 権威はユーザー設定だけ。ツリーやプロジェクトの config の `tree.*` は無視される

## 秘密を引く(haj secret)と自ツリーのデータ(haj store)

**所有の規律**: secret = 読みのみ(他所の所有物も含む)/ store = 読み書き
(自分の物だけ)。書きたい秘密は自分の store に置く。

```sh
# ツリーのコマンドの中(HAJ_TREE はコアが注入している)
client_secret=$(haj secret get CLIENT_SECRET)          # 宣言を解決(宣言に無い KEY はエラー)
token="${MM_TOKEN:-$(haj secret get MM_TOKEN)}"        # --secret MM_TOKEN=... が勝てる定石

printf '%s' "$refresh_token" | haj store put token     # 自分の名前空間に書く
token=$(haj store get token)
```

- store は常に自分の名前空間 `<prefix>/trees/<HAJ_TREE>/`(既定 prefix は
  `secret/data/users/<ユーザー名>`。`store.tree.prefix` で変更可)。引数は裸の
  論理パス。`store://` 前置きも受ける(設定から受けた参照データをそのまま渡せる)
- put は stdin 限定(argv の値は `ps` に見える)。フィールド単位の patch で、
  既にフィールドが在れば `--force` 無しでは拒否
- 宣言の一覧は `haj secret list`、検証は `haj secret check`(金庫に触らない)。
  人手の点検は `--tree <インストール名>` で対象を明示できる(get には無い —
  値に触る操作は文脈のみ)。ログインは `haj store login`、状態は `haj store status`
- そのインスタンスの全景(tree設定の実効値と出所・宣言・store の名前空間・
  各コマンドの `--haj-env` による既定値込みの実効 env)は
  `haj config --tree <インストール名>`
- **物理参照の口は無い。** 点検・横断・移行はツリー文脈の外で・人の明示で:
  `HAJ_TREE=<名前> haj store get token` /
  `haj --secret V=vault://... sh 'printf "%s\n" "$V"'` / エンジンの CLI

## エイリアスと組み合わせる(定番)

```
# ~/.config/haj/config — 「金庫の資格情報で起動するコマンド」を1語にする
alias.oci = --secret OCI_CLI_USER=vault://users/me/oci/user \
            --secret-file OCI_CLI_KEY_FILE=vault://users/me/oci/private_key \
            exec oci
alias.oci.desc = OCI CLI を金庫の資格情報で起動する
```

`haj oci iam region list` — 実体はどこにも残らず、補完も `oci` 自身に委譲される。

## 注意

- **シークレットは環境変数で渡すこと。** argv に展開すると `ps` から見える
- 規約フック(`--haj-describe` 等)には展開されない(TABのたびに金庫へ
  問い合わせない)。展開は本体を実行するときだけ
- キャッシュしない。毎回聞く(セッション管理は各CLIの仕事)

仕様の全文は `haj docs spec` の §10。旧 `haj secrets --check` は `haj secret check` に改名された。
