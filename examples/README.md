# examples

`.haj/commands/deploy` は、規約([SPEC.md](../SPEC.md))に従うサブコマンドの見本。

このディレクトリの中で `haj` を叩くと、`deploy` が生えているのが分かる。

```console
$ cd examples
$ haj
 hajコマンド (haj help <名前> で詳細):
   deploy    アプリをデプロイする

$ haj help deploy
haj deploy <staging|production> [--dry-run]
...

$ haj which deploy
/.../examples/.haj/commands/deploy

$ cd ..            # リポジトリの外に出ると deploy は消える
$ haj
コマンドが1つも見つかりません。
```

見本が示しているのは次の3点。

1. **規約フックは本体の初期化より前に処理する。** `--haj-describe` は `haj help` と
   TAB のたびに呼ばれるので、ここで重い初期化をすると TAB が遅くなる。
2. **`--haj-complete` の語数。** 渡ってくるのは「入力済みの語」なので、
   `$#` が 0 なら 1 語目を補完する。`[ $# -le 1 ]` と書くと 1 つずれる。
3. **共通ライブラリは `$HAJ_ROOT/lib/` から読む。** コアがツリーを教えてくれるので、
   プロジェクト固有のコマンドは自分のプロジェクトの lib を読むことになる。
