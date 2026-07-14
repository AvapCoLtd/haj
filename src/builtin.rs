//! コア組み込みのコマンド(SPEC.md §9)。
//!
//! `help` / `commands` / `which` / `selfupgrade` は探索の対象ではなく、コアが自分で
//! 処理する。だが**どこにいても常に使える**以上、一覧にも補完にも出さなければ嘘になる。
//! 「`haj help` の一覧が実態と一致する」ことが haj の売りなので、組み込みだけ
//! 見えないのは看過できない。
//!
//! サブコマンドと同じ形(名前・一行説明・詳しい使い方・補完候補)で扱えるように、
//! 規約フックに相当するものをここに持つ。違いは「プロセスを起動せずに答える」だけ。

use crate::selfupgrade;

pub struct Builtin {
    pub name: &'static str,
    pub describe: &'static str,
}

/// 予約語(discovery::is_reserved)と必ず一致させること。
/// ここに足したら向こうにも足す。ズレると「一覧に出るのに実行できない」コマンドが生まれる。
pub const ALL: &[Builtin] = &[
    Builtin {
        name: "help",
        describe: "使い方を表示する (haj help <名前> で個別)",
    },
    Builtin {
        name: "commands",
        describe: "コマンド一覧を機械可読で出す",
    },
    Builtin {
        name: "which",
        describe: "どの定義が効いているかを見る (--all で隠れているものも)",
    },
    Builtin {
        name: "config",
        describe: "設定の実効値と、その出所を見る",
    },
    Builtin {
        name: "selfupgrade",
        describe: "haj自身を更新する",
    },
    Builtin {
        name: "secrets",
        describe: "シークレット参照の展開対象を確認する (dry-run)",
    },
];

pub fn find(name: &str) -> Option<&'static Builtin> {
    ALL.iter().find(|b| b.name == name)
}

/// `haj help <組み込み>` の中身。
pub fn long_help(name: &str) -> Option<String> {
    Some(match name {
        "help" => "\
haj help [<名前>] — 使い方を表示する。

  haj help           現在のプロジェクトと、使えるコマンドの一覧
  haj help <名前>     そのコマンドの詳しい使い方

一覧は各コマンドの --haj-describe を聞いて自動生成される。手で書いた一覧が
実態とズレる、ということが起きない。"
            .to_string(),

        "commands" => "\
haj commands — コマンド一覧を機械可読で出す。

  名前 <TAB> パス <TAB> 出自 <TAB> 一行説明

スクリプトから haj のコマンドを列挙したいときに使う。人間向けの一覧は haj help。"
            .to_string(),

        "which" => "\
haj which [--all] <名前> — どの定義が効いているのかを見る。

  haj which setup          実行される実行ファイルのパスを出す
  haj which --all setup    同名の候補を探索順に全部出す(* が実行されるもの)

探索順は cwd に依存する。同名のコマンドがプロジェクトと共通の両方にあるとき、
どちらが走るのかを確かめるためにある。setup や reset は破壊的なので、
迷ったら実行する前にこれで確認すること。"
            .to_string(),

        "config" => format!(
            "\
haj config — 設定の実効値と、その出所を見る。

  ~/.config/haj/config  (XDG。$XDG_CONFIG_HOME を見る)

形式は key = value。'#' から行末はコメント。.haj/project と同じ形式なので、
覚えることは1つで済む。

  gitlab     = https://gitlab.avaper.day
  project_id = 788
  target     = {target}
  token      = glpat-xxxxxxxx

値は 環境変数 > 設定ファイル > 既定値 の順で決まる。この3段が見えないと
「なぜ効かないのか」を調べる手段が無くなるので、haj config は必ず出所を言う
(haj which が探索順を見せるのと同じ理由)。

token は値を出さない。設定されているかと、どこから来たかだけを出す。",
            target = crate::selfupgrade::DEFAULT_TARGET
        ),

        "selfupgrade" => selfupgrade::long_help(),

        "secrets" => "\
haj secrets — シークレット参照の展開対象を、解決せずに確かめる (dry-run)。

環境変数の値のうち、参照 (op:// / vault:// / {{ with secret ... }} / env:// / file://)
になっているものを列挙する。値は解決しない。金庫にも問い合わせない。

展開そのものは HAJ_SECRETS=1 のときだけ、サブコマンドを実行する exec の直前に行われる。
解決に失敗したら本体は実行されない (fail-fast)。詳細は SPEC.md §10。"
            .to_string(),

        _ => return None,
    })
}

/// 組み込みコマンドの補完候補。`words` は入力済みの語(サブコマンドの規約と同じ)。
pub fn complete(name: &str, words: &[String]) -> Vec<String> {
    match name {
        // haj help <TAB> / haj which <TAB> → コマンド名を出す
        "help" | "which" => {
            if !words.is_empty() && name == "help" {
                return Vec::new(); // help は引数1つだけ
            }
            let mut cands: Vec<String> = Vec::new();
            if name == "which" {
                if words.iter().any(|w| w.starts_with('-')) {
                    // --all は指定済み。あとはコマンド名
                } else if words.is_empty() {
                    cands.push("--all".to_string());
                }
                if words.iter().any(|w| !w.starts_with('-')) {
                    return Vec::new(); // コマンド名は指定済み
                }
            }
            cands.extend(names());
            cands
        }
        "selfupgrade" => {
            if words.is_empty() {
                vec!["--check".to_string()]
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

/// 補完に出す全コマンド名(探索で見つかるもの + 組み込み)。
fn names() -> Vec<String> {
    let mut v: Vec<String> = crate::discovery::list()
        .into_iter()
        .map(|c| c.name)
        .collect();
    v.extend(ALL.iter().map(|b| b.name.to_string()));
    v.sort();
    v
}
