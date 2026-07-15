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
        name: "env",
        describe: "コマンドが読む環境変数を key=value で出す (--env-file にそのまま渡せる)",
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
        describe: "何が渡るのかを解決せずに確かめる (--check)",
    },
    Builtin {
        name: "exec",
        describe: "PATHのコマンドにシークレットを注入して実行する",
    },
    Builtin {
        name: "docs",
        describe: "ドキュメントを読む (コマンドの作り方・仕様・ツリーの文書)",
    },
    Builtin {
        name: "completion",
        describe: "シェル補完のスクリプトを出す (eval \"$(haj completion zsh)\")",
    },
    Builtin {
        name: "sh",
        describe: "シェルの1行をシークレットを注入して実行する (exec sh -c の省略形)",
    },
    Builtin {
        name: "tree",
        describe: "共有ツリーの取得と更新 (install/update/list/remove)",
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

        "env" => "\
haj env <名前> — そのコマンドが読む環境変数を KEY=value で出す。

  haj env metrics > env.txt      雛形をファイルへ
  vi env.txt                     値を書き換える(シークレット参照も書ける)
  haj --env-file env.txt metrics 書き換えた値で実行する

中身はコマンド自身の --haj-env に聞くだけ(SPEC §4.4)。対応していない
コマンドではエラーになる。"
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

形式は key = value。'#' から行末はコメント。.haj/config と同じ形式なので、
覚えることは1つで済む。

  selfupgrade.token = vault://<マウント>/<パス>/token
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
--secret / --env-file / --secret-file も同様に効く。

  haj --secret DB_HOST=vault://secret/data/db/host exec sh -c 'mysql -h $DB_HOST'

シェルの変数展開 ($VAR) が要るなら、明示的に sh -c を書くか haj sh を使うこと。
haj は文字列をシェルに包まない。HAJ_ROOT / HAJ_NAME / HAJ_PROJECT は渡さない。
詳細は SPEC.md §9.2。"
            .to_string(),

        "sh" => "\
haj sh '<コマンド>' [引数...] — シェルの1行を、シークレットを注入して実行する。

haj exec sh -c '<コマンド>' の省略形。追加の引数は位置パラメータ ($1...) になる。
'--' で始めると、以降の語を空白で繋いで1行にする (ssh 方式)。

  haj --secret MYSQL_HOST=vault://secret/data/db/host sh 'mysql -h $MYSQL_HOST'
  haj sh 'echo $1-$2' one two    → one-two
  haj sh -- ls -la               → ls -la

展開の規則・環境変数の扱い (HAJ_PROJECT は渡る) は haj exec と同じ。詳細は SPEC.md §9.2。"
            .to_string(),

        "tree" => "\
haj tree — 共有ツリーの取得と更新 (SPEC §9.5)。

  haj tree install <gitのURL>[@<ref>] [--name <名前>]
  haj tree update [<名前>]       差分を見せてから ff-only で更新 (省略で全部)
  haj tree list                  名前 / 版 / コマンド数 / URL
  haj tree remove <名前>

git リポジトリを ~/.local/share/haj/trees/<名前> に置くだけ。入れたツリーは
探索の対象になり、一覧に [<名前>] として出る。探索順は
プロジェクト > 個人 > ツリー > 共通 ($HAJ_COMMAND_PATH)。

ツリーの根はリポジトリの .haj/ (あれば) かルート。commands/ が無いものは
ツリーとして認めない。git は CLI に委譲する (git が必要)。"
            .to_string(),

        "docs" => "\
haj docs [<トピック>] — 端末で読めるドキュメント。

  haj docs                    トピック一覧 (出自つき)
  haj docs writing-commands   コマンドの作り方 (haj同梱)
  haj docs spec               コアとサブコマンドの契約の全文 (haj同梱)

ツリーは commands/ と並んで docs/ を持てる。<ツリー>/docs/<トピック>.md を置けば、
コマンドと同じ探索でそのプロジェクトの中でだけ生える (説明は先頭の見出し行から取る)。
同名は手前が勝ち、haj 同梱のものはツリー側で上書きできる。

出力は素の markdown。長いものはページャへ: haj docs spec | less"
            .to_string(),

        "completion" => format!(
            "\
haj completion <シェル> — シェル補完のスクリプトを標準出力に出す。

  # ~/.zshrc  (bash なら ~/.bashrc に bash 版を)
  eval \"$(haj completion zsh)\"

対応シェル: {}

候補はスクリプトが持たない。コアの haj __complete に聞くだけなので、コマンドを
足しても補完は自動で追従する (プロジェクト固有のコマンドもそのまま出る)。",
            crate::completion::SHELLS.join(" / ")
        ),

        "selfupgrade" => selfupgrade::long_help(),

        "secrets" => "\
haj secrets --check — 何が渡るのかを、解決せずに確かめる (dry-run)。

グローバルフラグで渡そうとしているものを列挙する。参照の対象 (パス) は出すが、
値は解決しない。金庫に問い合わせないので、OIDC ログインもタッチ認証も起きない。

  haj --secret DB_PASS=vault://secret/data/db/password \\
      --env-file ./mig.env \\
      secrets --check

シークレットは**人が明示的に渡すものだけ**。haj は環境変数を勝手に走査しない
(渡すものと相手をその実行時に明示する)。

  --secret <名前>=<値>                       展開して環境変数で渡す (参照でなければ平文のまま)
  --env-file <ファイル>                       key = value を読み、値を展開して渡す
  --secret-file <名前|パス>=<参照|テンプレート>  値をファイルに書く (下記)

--secret-file は「ファイルで渡せ」と要求するツール向け。

  右辺が参照        → その値がファイルの中身になる
  右辺がそれ以外     → テンプレートファイルとみなして描画する
  左辺が名前        → 一時ファイルに書き、そのパスを環境変数 <名前> に入れる
                       (環境変数として妥当な名前のときだけ。KEY / KUBECONFIG など)
  左辺がパス        → そこに書く (config.ini / ~/.npmrc など。場所を固定要求するツール向け)

  haj --secret-file KEY=vault://secret/data/ssh/id_rsa sh 'ssh -i \"$KEY\" host'
  haj --secret-file KUBECONFIG=vault://secret/data/k8s/config exec kubectl get pods
  haj --secret-file ~/.npmrc=vault://secret/data/npm/rc exec npm publish
  haj --secret-file config.ini=config.ini.tpl app run     # テンプレート描画

一時ファイルは $XDG_RUNTIME_DIR (無ければ $TMPDIR) の 0700 ディレクトリに 0600 で
作る。cwd には決して書かない (リポジトリに commit される事故を防ぐ)。exec モデル上、
haj は後始末できないので消えない。

解決に失敗したらコマンドは実行されない (fail-fast)。詳細は SPEC.md §10。"
            .to_string(),

        _ => return None,
    })
}

/// 組み込みコマンドの補完候補。`words` は入力済みの語(サブコマンドの規約と同じ)。
pub fn complete(name: &str, words: &[String]) -> Vec<String> {
    match name {
        // haj help <TAB> / haj which <TAB> → コマンド名を出す
        "help" | "which" | "env" => {
            if !words.is_empty() && name != "which" {
                return Vec::new(); // help / env は引数1つだけ
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
        "docs" => crate::docs::complete(words),
        "completion" => crate::completion::complete(words),
        "tree" => crate::tree::complete(words),
        "secrets" => {
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
