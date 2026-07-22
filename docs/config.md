# 設定リファレンス(書ける鍵のすべて)

設定は `key = value` の1形式だけ。`#` から行末はコメント、行末の `\` は継続。
鍵は git と同じドット記法で名前空間を持ち、ドット無しはコア。

雛形は自分で書かなくてよい — 設定できる鍵と既定値は `haj config --init` が
**すべて**出す(全行コメントなので、そのまま置いても挙動は変わらない):

```sh
haj config --init > ~/.config/haj/config
```

## 場所

| 何 | 場所 |
|---|---|
| ユーザー設定 | `~/.config/haj/config`(XDG。`$XDG_CONFIG_HOME` を見る) |
| プロジェクト設定 | `<リポジトリ>/.haj/config` |
| ツリーの config | `<ツリー>/config`(配布物。ただし `tree.*` は書けない — 下記) |

## 優先順位

**環境変数 > 設定ファイル > 既定値。** 実効値と出所は `haj config` が必ず両方出す
(なぜ効かないのかを調べる手段を残すため)。`token` 系は値を出さず、設定の有無と
出所だけを示す。

## ユーザー設定の鍵(~/.config/haj/config)

### コア

| 鍵 | 環境変数 | 既定 | 意味 |
|---|---|---|---|
| `command_path` | `HAJ_COMMAND_PATH` | `/usr/local/lib/haj/commands` | システム共通のコマンド置き場(`:` 区切り) |
| `hook_timeout_ms` | `HAJ_HOOK_TIMEOUT_MS` | `2000` | 規約フックのタイムアウト |

### エイリアス

| 鍵 | 意味 |
|---|---|
| `alias.<名前>` | 1行の委譲。グローバルフラグから書き始められる |
| `alias.<名前>.desc` | `haj help` の一覧に出る説明 |

### Vault / 1Password 連携(secrets.*)

| 鍵 | 環境変数 | 既定 | 意味 |
|---|---|---|---|
| `secrets.vault_cmd` | `HAJ_VAULT_CMD` | `vault` | vault 参照の解決に使う CLI(OpenBao なら `bao`) |
| `secrets.vault_addr` | `VAULT_ADDR` | (無し) | 接続先。環境の `VAULT_ADDR` / `BAO_ADDR` が優先 |
| `secrets.vault_cert_login` | `HAJ_VAULT_CERT_LOGIN` | (無し) | 未ログイン時、OIDC より先に試す cert 認証の委譲先コマンド |
| `secrets.vault_login` | `HAJ_VAULT_LOGIN` | `off` | 未ログイン時に自動実行する `login` の引数。`off` で無効 |
| `secrets.op_cmd` | `HAJ_OP_CMD` | `op` | op 参照の解決に使う CLI |

未ログイン時の自動ログインは**連鎖**する: token lookup → cert 委譲(設定があれば)→
OIDC。認証しない CI では `HAJ_VAULT_LOGIN=off` を置くこと。

### 秘密・テンプレートの宣言(user.* / tree.*)

**設定ファイル専用**(対応する環境変数は無い)。値は書かず、参照だけを書く:

| 鍵 | 意味 |
|---|---|
| `user.secret.KEY` | ユーザー文脈の秘密の宣言。ツリーの外でだけ引ける |
| `user.template.KEY` | テンプレート宣言。値は tpl ファイルのパス |
| `tree.<インストール名>.env.KEY` | ツリーごとの設定注入(平文をそのまま。展開しない) |
| `tree.<インストール名>.secret.KEY` | ツリーの秘密の宣言。注入されない — `haj secret get` で引く |
| `tree.<インストール名>.template.KEY` | ツリーのテンプレート宣言 |

権威はユーザー設定だけ — ツリーやプロジェクトの config に書かれた `tree.*` は
無視される。詳細は [シークレット](secrets.md)。

### ストア(store.*)

| 鍵 | 環境変数 | 既定 | 意味 |
|---|---|---|---|
| `store.tree.engine` | `HAJ_STORE_TREE_ENGINE` | `vault` | ストア `tree` のエンジン(v1 は vault のみ) |
| `store.tree.prefix` | `HAJ_STORE_TREE_PREFIX` | `secret/data/users/<ユーザー名>` | 物理プレフィックス |

### 更新(selfupgrade.*)

| 鍵 | 環境変数 | 既定 | 意味 |
|---|---|---|---|
| `selfupgrade.github` | `HAJ_GITHUB` | `AvapCoLtd/haj` | 取得元の GitHub リポジトリ(public。認証不要) |
| `selfupgrade.target` | `HAJ_TARGET` | `x86_64-unknown-linux-musl` | 取得するビルドのターゲット |
| `selfupgrade.gitlab` | `HAJ_GITLAB` | (無し) | private な取得元(GitLab)を使うとき |
| `selfupgrade.project_id` | `HAJ_PROJECT_ID` | (無し) | 同上。プロジェクト ID |
| `selfupgrade.token` | `HAJ_TOKEN` | (無し) | 同上。**シークレット参照を書ける**(`vault://...`) |

`selfupgrade.token` の参照は token を使うときだけ解決される。展開されるのはこの鍵
**だけ** — リゾルバ自身の設定(`secrets.*`)は展開しない(参照の解決に参照が要る
再帰を作らない)。

### docs の表示(docs.*)

| 鍵 | 環境変数 | 既定 | 意味 |
|---|---|---|---|
| `docs.fzf_cmd` | `HAJ_DOCS_FZF_CMD` | `fzf` | 選択 UI に使うコマンド |
| `docs.fzf_args` | `HAJ_DOCS_FZF_ARGS` | (無し) | 選択 UI へ追加で渡す引数(後勝ちで上書きできる) |
| `docs.preview_cmd` | `HAJ_DOCS_PREVIEW_CMD` | (無し) | プレビューのフィルタ(例: `glow -`) |
| `docs.pager` | `HAJ_DOCS_PAGER` | `$PAGER`、無ければ `less` | Enter で開くビューア |

### ユーザー定義域(meta.*)

コアは一切解釈しない。ツリー間で共有する「本人についての値」の置き場:

```
meta.username = hajime    # Vault でのユーザー名 (OS のユーザー名と違いうる)
```

スクリプトからは `haj config get meta.username` / `haj config set meta.username <名前>`。
`set` の保存先は常にユーザー設定 — 人がコマンドを打つこと自体が同意。

## プロジェクト設定の鍵(.haj/config)

| 鍵 | 意味 |
|---|---|
| `name` | プロジェクト名(`HAJ_PROJECT` として渡る。無ければディレクトリ名) |
| `root` | 既定 `true`。`false` で境界の壁を開け、親のコマンドも継承する(モノレポ用) |
| `alias.<名前>` | プロジェクト局所のエイリアス |
| `task.<名前>` / `.desc` | タスク(1行の委譲)。`haj run <名前>` で実行 |

ツリーの config はこれに加えて `expose`(`flat` / `namespace`)を持つ
([ツリーの作り方と配布](trees.md))。

## 環境変数だけのもの(設定ファイルに書けない)

| 変数 | 意味 |
|---|---|
| `HAJ_NO_CACHE` | `1` で説明文キャッシュを無効化(デバッグ用) |
| `XDG_CONFIG_HOME` / `XDG_CACHE_HOME` | 置き場所そのものを決めるので、設定ファイルには書けない |

規範としての全文(継続行の規則、置換の挙動など)は `haj docs spec` の §8。