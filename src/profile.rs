//! ビルドプロファイル(SPEC §8.5)。
//!
//! 素のビルド(`cargo build`)は**公開プロファイル** — avap のインフラを一切
//! 向かない中立な既定値になる。GitHub からソースを取った人が `cargo build` した
//! バイナリに、社内の vault や GitLab の URL が焼き込まれていてはいけない。
//!
//! avap の値は、CI がビルド時に `HAJ_BUILD_AVAP=1` を注入したときだけ
//! 焼き込まれる(option_env! はコンパイル時に評価され、cargo が依存として追跡する)。
//! 実行時の環境変数ではないことに注意。
//!
//! 判定は「定義されているか」だけ(値は見ない)。const 文脈では文字列比較が
//! まだ安定化されていないため。

/// avap プロファイルでビルドされているか。
pub const AVAP: bool = option_env!("HAJ_BUILD_AVAP").is_some();

/// プロファイルで値を出し分ける。
pub const fn pick(avap: &'static str, public: &'static str) -> &'static str {
    if AVAP {
        avap
    } else {
        public
    }
}
