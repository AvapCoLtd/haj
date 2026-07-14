//! `haj completion <シェル>` — 補完スクリプトを標準出力に出す(SPEC.md §9.4)。
//!
//! 補完スクリプトをバイナリに埋め込む(`include_str!`)。ファイルとして配る必要が
//! なくなり、バイナリ1本で完結する(gh / kubectl / rustup と同じ流儀)。
//!
//!   # ~/.zshrc
//!   eval "$(haj completion zsh)"
//!
//! 中身はコアの `haj __complete` に聞くだけなので、コマンドを足しても補完スクリプトは
//! 変わらない(SPEC §6)。埋め込み元は completions/ のファイルそのもの。

const ZSH: &str = include_str!("../completions/_haj");
const BASH: &str = include_str!("../completions/haj.bash");

/// 対応するシェル。増やすときはここと SHELLS だけ。
pub const SHELLS: &[&str] = &["zsh", "bash"];

pub fn run(args: &[String]) -> ! {
    let Some(shell) = args.first() else {
        eprintln!("haj: 使い方: haj completion <{}>", SHELLS.join("|"));
        std::process::exit(1);
    };

    let script = match shell.as_str() {
        "zsh" => ZSH,
        "bash" => BASH,
        other => {
            eprintln!(
                "haj: 対応していないシェルです: {other} (対応: {})",
                SHELLS.join(", ")
            );
            std::process::exit(1);
        }
    };
    print!("{script}");
    std::process::exit(0);
}

/// `haj completion <TAB>` の候補。
pub fn complete(words: &[String]) -> Vec<String> {
    if words.is_empty() {
        SHELLS.iter().map(|s| s.to_string()).collect()
    } else {
        Vec::new()
    }
}
