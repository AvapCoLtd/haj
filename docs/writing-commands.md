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
  (`help` `commands` `which` `config` `exec` `sh` `selfupgrade` `secrets` `docs` `__complete`)

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
モノレポのサブプロジェクトで親のコマンドも継承したいときだけ、`.haj/project` に
`root = false` と書いて壁を開ける。

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
esac

# ---- ここから本体。重い初期化はフックの後に置く(TABの速さに直結する) ----
echo "本処理"
```

フックの制約(SPEC §4.4):

- **stdin は /dev/null**。入力を待ってはならない
- **stderr は捨てられる**。タイムアウトは既定2秒 — 超えると SIGKILL
- **副作用禁止**。フックは `haj help` と TAB のたびに呼ばれる
- `--haj-describe` を実装しなくても動く(一覧の説明が空になるだけ)。ただし必須とする

候補を列挙できない箇所(自由入力)では丸括弧の1行を返すと、補完はそれを説明として出す:

```sh
--haj-complete) shift; [ $# -eq 1 ] && echo "(新しいマイグレーションの名前)"; exit 0 ;;
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
haj --secret DB_PASS=vault://avap/data/db/password mig up
# コマンド側は $DB_PASS を読むだけ。bao の存在を知らなくてよい
```

- 値の置き場所は3層: `--secret`/`--env`/`--secretfile`(明示)、環境変数の走査(`HAJ_SECRETS=1`)、`.haj/env`(予定)
- 解決に失敗するとコマンドは**実行されない**(fail-fast)。未解決の参照文字列が渡ってくる心配はしなくてよい
- 設定ファイルが要るツールには `--secretfile 出力=テンプレート.tpl` で描画して渡せる

## 6. デバッグ

```sh
haj which --all setup     # どの定義が勝っていて、何が隠れているか
haj commands              # 機械可読の一覧(名前/パス/出自/説明)
HAJ_NO_CACHE=1 haj help   # 説明文キャッシュを無効化して聞き直す
HAJ_HOOK_TIMEOUT_MS=10000 haj help   # 遅いフックの調査
haj -C ~/repos/x help     # 別プロジェクトを起点に見る
```

「説明が一覧に出ない」ときは、まず `HAJ_NO_CACHE=1` で聞き直し、次に
`.haj/commands/<名前> --haj-describe` を直接叩いて、1行目に説明が出て exit 0 かを確かめる。

## 7. やってはいけないこと

- **予約語の名前でコマンドを置く** — 無視される(`exec` や `sh` を奪えるとシークレットの横取りができてしまうため)
- **フックで対話やネットワークを待つ** — TAB のたびに全員が待たされ、2秒で殺される
- **コマンド一覧を help.header に手書きする** — 一覧は自動生成される。手書きは必ず実態とズレる
- **`haj.toml` 的な宣言でタスクを書きたくなる** — haj は意図的にそれをやらない(SPEC §11)。分岐や冪等性を含む現実のタスクは、最初からスクリプトとして書く

## 8. 実例

このリポジトリの `examples/.haj/commands/deploy` に、規約フック・引数解釈・
`HAJ_ROOT` の利用まで含めた実物がある。困ったら `haj docs spec` で契約の全文を引ける。
