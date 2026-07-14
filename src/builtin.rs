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
        describe: "設定の実効値と出所を見る (--init で雛形を出す)",
    },
    Builtin {
        name: "selfupgrade",
        describe: "haj自身を更新する",
    },
    Builtin {
        name: "secrets",
        describe: "シークレット参照の展開対象を確認する (dry-run)",
    },
    Builtin {
        name: "exec",
        describe: "PATHのコマンドにシークレットを注入して実行する",
    },
    Builtin {
        name: "sh",
        describe: "シェルの1行をシークレットを注入して実行する (exec sh -c の省略形)",
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

        "config" => "\
haj config — 設定の実効値と、その出所を見る。

  haj config           実効値と出所の一覧
  haj config --init    設定できる鍵と既定値をすべて、雛形として出す(全行コメント)

雛形はそのままリダイレクトすれば初期化になる:

  haj config --init > ~/.config/haj/config

  ~/.config/haj/config  (XDG。$XDG_CONFIG_HOME を見る)

形式は key = value。'#' から行末はコメント。.haj/project と同じ形式なので、
覚えることは1つで済む。

  selfupgrade.token = vault://users/<名前>/gitlab-pat/gitlab.avaper.day/token
  secrets.vault_cmd = bao

鍵は git と同じドット記法で名前空間を持つ (selfupgrade.* / secrets.* / ドット無しはコア)。

値は 環境変数 > 設定ファイル > 既定値 の順で決まる。この3段が見えないと
「なぜ効かないのか」を調べる手段が無くなるので、haj config は必ず出所を言う
(haj which が探索順を見せるのと同じ理由)。

selfupgrade.token の実体は出さない (参照なら参照を出す)。"
            .to_string(),

        "exec" => "\
haj exec <プログラム> [引数...] — PATH のコマンドにシークレットを注入して実行する。

探索は通さない。「注入は欲しいが、haj のコマンドにするほどではない」一回きりの
実行のためにある (op run / doppler run の位置)。展開の規則はサブコマンド実行と同じで、
--secret / --env / --secretfile も HAJ_SECRETS=1 の走査も同様に効く。

  haj --secret DB_HOST=vault://avap/data/db/host exec sh -c 'mysql -h $DB_HOST'

シェルの変数展開 ($VAR) が要るなら、明示的に sh -c を書くか haj sh を使うこと。
haj は文字列をシェルに包まない。HAJ_ROOT / HAJ_NAME / HAJ_PROJECT は渡さない。
詳細は SPEC.md §9.2。"
            .to_string(),

        "sh" => "\
haj sh '<コマンド>' [引数...] — シェルの1行を、シークレットを注入して実行する。

haj exec sh -c '<コマンド>' の省略形。追加の引数は位置パラメータ ($1...) になる。
'--' で始めると、以降の語を空白で繋いで1行にする (ssh 方式)。

  haj --secret MYSQL_HOST=vault://avap/data/db/host sh 'mysql -h $MYSQL_HOST'
  haj sh 'echo $1-$2' one two    → one-two
  haj sh -- ls -la               → ls -la

展開の規則・HAJ_* を渡さないことは haj exec と同じ。詳細は SPEC.md §9.2。"
            .to_string(),

        "selfupgrade" => selfupgrade::long_help(),

        "secrets" => "\
haj secrets — シークレット参照の展開対象を、解決せずに確かめる (dry-run)。

環境変数の値のうち、参照 (op:// / vault:// / {{ with secret ... }} / env:// / file://)
になっているものを列挙する。値は解決しない。金庫にも問い合わせない。

展開そのものは HAJ_SECRETS=1 のときだけ、サブコマンドを実行する exec の直前に行われる。
解決に失敗したら本体は実行されない (fail-fast)。

渡すものと相手をその実行時に明示するには、サブコマンド名の**前**のフラグを使う
(こちらは HAJ_SECRETS 不要。フラグを打ったこと自体が同意):

  haj --secret DB_PASS=vault://avap/data/db/password \\
      --env ./mig.env \\
      --secretfile config.ini=config.ini.tpl \\
      mig up

  --secret <名前>=<値>              展開して環境変数で渡す(参照でなければ平文のまま)
  --env <ファイル>                  key = value を読み、値を展開して渡す
  --secretfile <出力>=<テンプレート>  描画して 0600 で書いてから実行(haj は消さない)

詳細は SPEC.md §10。"
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
        "config" => {
            if words.is_empty() {
                vec!["--init".to_string()]
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
