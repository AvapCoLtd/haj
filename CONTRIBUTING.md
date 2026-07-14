# 開発

依存クレートは**ゼロ**。標準ライブラリだけで書く、というのが設計上の制約。
haj がやるのは探索と exec だけで CPU の仕事は無く、clap や serde を持ち込んでも
ビルド時間と監査対象が増えるだけで得るものが無い。

```sh
cargo test                                 # 統合テスト
cargo clippy --all-targets -- -D warnings  # 警告はエラー扱い
cargo fmt --check
```

テストは一時ディレクトリに本物の実行ファイルを置いて `haj` を外から叩く。
探索順・同名の優先度・規約・フックのタイムアウト・終了コードの伝播といった、
この道具の本質そのものを検証している。

シークレット関連(`tests/secrets.rs`)は金庫に触らない。偽の `vault` / `op` / `curl` を
PATH に置いて差し替え、全経路を通す。

## 設計の原則

- **コアはディスパッチ表を持たない。** 名前から実行ファイルを探して exec するだけ
- **コアはサブコマンドの中身を知らない。** 説明・使い方・補完はすべて本人に聞く(規約)
- **一覧は実態と一致する。** 手で書いた一覧は必ずズレるので、自動生成する
- **素性は常に見える。** どの定義が効いているか、値がどこから来たかを必ず出す
- **タスクを宣言型ファイルに閉じ込めない。** 現実のタスクは分岐と冪等性の塊

契約の全文は [SPEC.md](SPEC.md)。実装より先に SPEC を更新すること。

## リリース

`Cargo.toml` の版を上げてタグを打つと、CI が全部やる。

```sh
git tag v0.13.0 && git push origin v0.13.0
```

タグ(`v` を除いた部分)と `Cargo.toml` の `version` が食い違うと CI は失敗する。

タグで走るもの:

1. **test** — fmt / clippy / cargo test
2. **build** — musl 静的バイナリ(x86_64 / aarch64)。静的リンクを ELF ヘッダで検証
3. **publish** — 社内 GitLab の Package Registry(社内向けの控え)
4. **mirror** — GitHub `AvapCoLtd/haj` へ push
5. **github-release** — GitHub Releases に tar.gz と .sha256 を添付(公開配布の顔)

## GitHub ミラー

GitLab が正(canonical)で、GitHub は公開のためのミラー。

認証は GitHub App「haj-release」(Contents: Read and write、`AvapCoLtd/haj` のみに
インストール)。App の installation token は1時間で失効するため、GitLab 組み込みの
push mirror ではなく CI ジョブ駆動にしている(`ci/github.sh`)。秘密鍵は bao に保管し、
CI 変数 `GH_APP_ID` / `GH_APP_PRIVATE_KEY`(file 型)で渡す。

`ci/github.sh` の依存は openssl / curl / git だけ(秘密鍵 → JWT → installation token)。
