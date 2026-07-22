//! haj — サブコマンドを探索して実行する、薄いルーター。
//!
//! コアはディスパッチ表を持たない。`haj mig` は「どこかにある実行可能ファイル
//! `mig`」を探して exec するだけ。プロジェクトごとに異なるサブコマンドの
//! サブセットは、リポジトリに `.haj/commands/` を置くことで自然に成立する。
//!
//! 仕様は SPEC.md を参照。

mod aliases;
mod builtin;
mod cache;
mod completion;
mod config;
mod contract;
mod discovery;
mod docs;
mod project;
mod secret;
mod secrets;
mod selfupgrade;
mod store;
mod tasks;
mod tree;

use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::Command as Proc;

use cache::DescribeCache;
use discovery::Command;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE_FLAGS: &str = "使い方: haj [-C <ディレクトリ>] [--secret <名前>=<値>]... [--env-file <ファイル>]... [--secret-file <名前|パス>=<参照|テンプレート>]... <コマンド> [引数...]";

fn main() {
    // Rust は起動前に SIGPIPE を「無視」にする。そのままだと (1) `haj help | head` の
    // ような早閉じで haj 自身の println! が EPIPE の panic になり、(2) exec(2) は
    // 「無視」の処分を子に引き継ぐため、サブコマンドまで SIGPIPE で死ねなくなる。
    // ルーターは C のコマンドと同じ顔をするべきなので、既定(パイプが閉じたら
    // 黙って死ぬ)に戻してから仕事を始める。
    reset_sigpipe();

    // 起動時の cwd を `-C` の適用前に記録する(HAJ_START_DIR、SPEC §3.1)。
    contract::record_start_dir();

    let mut args: Vec<String> = std::env::args().skip(1).collect();

    let mut deliveries: Vec<secrets::Delivery> = Vec::new();
    let mut alias_expanded = false;
    let mut task_expanded = false;

    // グローバルフラグの解釈と、エイリアス展開(SPEC §2.7)。
    // エイリアスは名前の位置で**1回だけ**語の並びに置き換わり(再帰しない)、
    // 展開結果の先頭にフラグがあれば通常どおり解釈し直す。
    let (name, rest): (String, Vec<String>) = loop {
        // haj自身のグローバルフラグ(SPEC §3.2 / §10.7)。**サブコマンド名の前にだけ**
        // 書ける。名前以降は解釈しない(§11)ので、フラグ以外に当たったらそこで止まる。
        let mut idx = 0;
        while idx < args.len() {
            let flag = args[idx].as_str();
            if !matches!(flag, "-C" | "--secret" | "--env-file" | "--secret-file") {
                break;
            }
            let Some(arg) = args.get(idx + 1) else {
                die(&format!("{flag} には値が要ります\n{USAGE_FLAGS}"));
            };
            if flag == "-C" {
                // git と同じ。この場で移動するので、探索・プロジェクト境界・HAJ_PROJECT・
                // サブコマンドの cwd がすべて移動先を起点になる。複数指定は順に適用され、
                // 相対パスは直前の -C からの相対(git と同一の意味論)。
                let dir = expand_home(arg);
                if let Err(e) = std::env::set_current_dir(&dir) {
                    die(&format!("-C {arg}: 移動できません: {e}"));
                }
            } else {
                match secrets::Delivery::parse(flag, arg) {
                    Ok(d) => deliveries.push(d),
                    Err(e) => die(&e),
                }
            }
            idx += 2;
        }

        match args[idx..].split_first() {
            None => {
                if !deliveries.is_empty() {
                    die(&format!(
                        "フラグの後にコマンド名がありません\n{USAGE_FLAGS}"
                    ));
                }
                // 素の `haj` はヘルプ。何も分からない状態で来た人に一覧を見せるのが親切。
                print_help(&[]);
                std::process::exit(0);
            }
            Some((n, r)) => {
                // 優先順位は git と同じ: 予約語(組み込み) > エイリアス > 探索。
                // 定義はプロジェクトの .haj/config とユーザー設定から(aliases 参照)。
                // 直前までに -C を適用済みなので、移動先のプロジェクトの定義が見える。
                if !alias_expanded && !n.starts_with('-') && !discovery::is_reserved(n) {
                    if let Some(a) = aliases::lookup(n) {
                        alias_expanded = true;
                        let mut expanded: Vec<String> =
                            a.expansion.split_whitespace().map(str::to_string).collect();
                        expanded.extend(r.iter().cloned());
                        args = expanded;
                        continue; // フラグから解釈し直す
                    }
                }
                // タスクの1行宣言(task.* — SPEC §9.6)も同じ機構で展開する。
                // 展開はエイリアスとタスクで各1回だけ(フラグは別) —
                // `alias.t = run test` は動くが、`task.a = run b` の b の宣言は
                // 再展開しない(エイリアスの「再帰しない」と同じ規則)。
                if !task_expanded && n == "run" {
                    if let Some((task_name, task_rest)) = r.split_first() {
                        if let Some(tasks::Task::Decl { expansion, .. }) =
                            tasks::lookup_decl(task_name)
                        {
                            task_expanded = true;
                            let mut expanded: Vec<String> =
                                expansion.split_whitespace().map(str::to_string).collect();
                            expanded.extend(task_rest.iter().cloned());
                            args = expanded;
                            continue; // フラグから解釈し直す
                        }
                    }
                }
                break (n.to_string(), r.to_vec());
            }
        }
    };
    let name = name.as_str();
    let rest: &[String] = &rest;

    // 受け渡しフラグは「本体を実行する」ときと、その dry-run のときにだけ意味がある。
    // それ以外の組み込みに続けて書かれたら使い方の誤り(SPEC §10.2)。
    if !deliveries.is_empty()
        && !matches!(name, "exec" | "sh" | "secret" | "secrets" | "run")
        && (discovery::is_reserved(name) || name.starts_with('-'))
    {
        die(&format!(
            "--secret / --env-file / --secret-file は <コマンド> の実行時にだけ使えます\n{USAGE_FLAGS}"
        ));
    }

    match name {
        // 探索を通さず、PATH のコマンドに注入だけして実行する。SPEC §9.2。
        "exec" => exec_external(rest, &deliveries),
        // exec sh -c の省略形。シェルの変数展開($VAR)を1語で使えるように。
        "sh" => exec_shell(rest, &deliveries),
        // プロジェクトのタスク(SPEC §9.6)。探索しない・上書きしない・フォールバックしない。
        // 1行宣言(task.*)は上の展開ループが処理済みなので、ここに来るのは
        // 実行ファイルのタスクと一覧。
        "run" => run_task(rest, &deliveries),
        "-h" | "--help" | "help" => {
            print_help(rest);
            std::process::exit(0);
        }
        "-V" | "--version" => {
            println!("haj {VERSION}");
            std::process::exit(0);
        }
        "config" => {
            // --init は設定ファイルの雛形を標準出力へ(全行コメント)。
            // そのままリダイレクトすれば初期化になる。SPEC §8.2。
            // --tree はそのインスタンスに効く設定の実効値と出所(SPEC §10.8)。
            if rest.first().map(String::as_str) == Some("--init") {
                config::template();
            } else if rest.first().map(String::as_str) == Some("get") {
                // plumbing(SPEC §8.5): 実効値を1行。未設定は exit 1
                let Some(key) = rest.get(1).filter(|k| !k.is_empty()) else {
                    die("使い方: haj config get <キー>");
                };
                if rest.len() > 2 {
                    die("引数が多すぎます: haj config get <キー>");
                }
                config::get_value(key);
            } else if rest.first().map(String::as_str) == Some("set") {
                // plumbing(SPEC §8.5): ユーザー設定へ書く(人が打つこと自体が同意)
                let (Some(key), Some(value)) = (rest.get(1).filter(|k| !k.is_empty()), rest.get(2))
                else {
                    die("使い方: haj config set <キー> <値>");
                };
                if rest.len() > 3 {
                    die("引数が多すぎます: haj config set <キー> <値> (空白を含む値は引用符で)");
                }
                config::set_value(key, value);
            } else if rest.first().map(String::as_str) == Some("--tree") {
                let Some(tree) = rest.get(1).filter(|t| !t.is_empty()) else {
                    die("--tree には値が要ります: haj config --tree <インストール名> (一覧: haj tree list)");
                };
                // 実効 env: そのツリーの全コマンドの --haj-env を節連結(SPEC §10.8)。
                // 金庫には触らないが規約フックは実行する — 既定値の権威はコマンド
                // 本人にあり、コアは聞くだけ(§4.4)。一覧は実態と一致(§2.4)。
                let eff =
                    tree::find(tree).and_then(|dir| env_sections(&tree::tree_commands(tree, &dir)));
                config::show_tree(tree, eff.as_deref());
            } else {
                config::show();
            }
            std::process::exit(0);
        }
        "selfupgrade" => selfupgrade::run(rest),
        // 共有ツリーの取得と更新。SPEC §9.5。
        "tree" => tree::run(rest),
        // 端末で読めるドキュメント。SPEC.md §9.3。
        "docs" => docs::run(rest),
        // 補完スクリプトを吐く。eval "$(haj completion zsh)" で使う。SPEC.md §9.4。
        "completion" => completion::run(rest),
        // 宣言された秘密を引く(get / list)と、宣言と受け渡しの検証(check)。
        // SPEC §10.6 / §10.9。文脈は自分の環境の HAJ_TREE。
        "secret" => secret::run(rest, &deliveries),
        // 旧名の移行スタブ(SPEC §10.6)。予約語には残す — 探索に明け渡すと、
        // 旧名で書かれたスクリプトが野良コマンドに落ちる。
        "secrets" => {
            eprintln!("haj: secrets は secret check に改名されました: haj secret check");
            std::process::exit(1);
        }
        // 自ツリーのストアの読み書きと認証。SPEC §10.10。
        // 引数は裸の論理パス。文脈は自分の環境の HAJ_TREE(ツリーのコマンドの
        // 中から `... | haj store put token` と合成できる)。
        "store" => store::run(rest),
        // コマンドが読む環境変数を中継する(--haj-env)。出力は --env-file に渡せる形式。
        // 「どの環境変数を読むのか」はコマンドの中身の知識なので、コアは聞くだけ。SPEC §4.4。
        "env" => {
            let Some(target) = rest.first() else {
                die("使い方: haj env <コマンド> / haj env run [<タスク>] / haj env <ツリー名> [<名前>]");
            };
            // タスク(SPEC §9.6): haj env run <名前> — --haj-env の中継はコマンドと同じ
            if target == "run" {
                task_env(rest.get(1).map(String::as_str));
            }
            // ツリー名前空間(SPEC §9.7): haj env <ツリー名> <名前>
            if let Some(dir) = tree::find(target) {
                tree_env(target, &dir, rest.get(1).map(String::as_str));
            }
            let Some(cmd) = discovery::resolve(target) else {
                eprintln!("haj: 未知のコマンドです: {target}");
                std::process::exit(1);
            };
            match env_report(&cmd) {
                Some(v) => println!("{v}"),
                None => {
                    eprintln!(
                        "haj: {target} は --haj-env に対応していません ({})",
                        cmd.path.display()
                    );
                    std::process::exit(1);
                }
            }
            std::process::exit(0);
        }
        // 機械向け。シェル補完から呼ばれる。SPEC.md「補完プロトコル」参照。
        "__complete" => {
            complete(rest);
            std::process::exit(0);
        }
        // 一覧を機械可読で。スクリプトから使えるように。
        "commands" => {
            let mut c = DescribeCache::load();
            for cmd in discovery::list() {
                let d = describe(&mut c, &cmd).unwrap_or_default();
                println!(
                    "{}\t{}\t{}\t{}",
                    cmd.name,
                    cmd.path.display(),
                    cmd.origin.label(),
                    d
                );
            }
            c.save();
            // 組み込みも出す。どこにいても使えるのだから、一覧から漏らしてはいけない。
            // パスは無いので "(組み込み)" と書く。
            for b in builtin::ALL {
                println!(
                    "{}\t(組み込み)\t{}\t{}",
                    b.name,
                    project::Origin::Core.label(),
                    b.describe
                );
            }
            std::process::exit(0);
        }
        // どの定義が勝っているかを見る。探索順が絡む以上、これが無いと調べようがない。
        "which" => which(rest),
        _ => {}
    }

    // ツリー名前空間(SPEC §9.7)。予約語 > エイリアス > ツリー名 > 探索 —
    // ツリー名が探索より手前なのは、名前空間が cwd に依存せずどこでも同じに動くため。
    if let Some(dir) = tree::find(name) {
        run_tree_namespace(name, &dir, rest, &deliveries);
    }

    let Some(cmd) = discovery::resolve(name) else {
        eprintln!("haj: 未知のコマンドです: {name}\n");
        print_usage();
        std::process::exit(127); // シェルの「command not found」に合わせる
    };

    exec_command(&cmd, rest, &deliveries)
}

/// `haj <ツリー名> [<名前>] [引数...]`(SPEC §9.7)— ツリー名前空間。
/// expose に関係なく全ツリーで使える明示形。run が「このプロジェクトの」を
/// 明示するのと同型で、こちらは「このツリーの」を明示する。
fn run_tree_namespace(
    tree_name: &str,
    dir: &std::path::Path,
    args: &[String],
    deliveries: &[secrets::Delivery],
) -> ! {
    let Some((cname, rest)) = args.split_first() else {
        list_tree_commands(tree_name, dir);
    };
    let Some(cmd) = tree::tree_command(tree_name, dir, cname) else {
        eprintln!("haj: {tree_name} に {cname} はありません (haj {tree_name} で一覧)");
        std::process::exit(127);
    };
    exec_command(&cmd, rest, deliveries)
}

/// `haj <ツリー名>`(引数なし)— そのツリーのコマンド一覧(run と同じ振る舞い)。
fn list_tree_commands(tree_name: &str, dir: &std::path::Path) -> ! {
    let cmds = tree::tree_commands(tree_name, dir);
    if cmds.is_empty() {
        println!("ツリー {tree_name} にコマンドはありません。");
        std::process::exit(0);
    }
    println!(" ツリー: {tree_name}  ({})", tree::tree_root(dir).display());
    println!("\n コマンド (haj {tree_name} <名前> で実行):");
    let mut cache = DescribeCache::load();
    let width = cmds.iter().map(|c| c.name.len()).max().unwrap_or(8);
    for c in &cmds {
        let d = describe(&mut cache, c).unwrap_or_default();
        println!("   {:width$}  {}", c.name, d, width = width);
    }
    cache.save();
    std::process::exit(0);
}

/// `haj env <ツリー名> [<名前>]`(SPEC §9.7)。名前なしは全コマンドの連結 —
/// そのツリーが読む環境変数の全景が1回で見える。
fn tree_env(tree_name: &str, dir: &std::path::Path, name: Option<&str>) -> ! {
    let Some(name) = name else {
        match env_sections(&tree::tree_commands(tree_name, dir)) {
            Some(v) => print!("{v}"),
            None => {
                eprintln!("haj: {tree_name} に --haj-env に対応するコマンドがありません");
                std::process::exit(1);
            }
        }
        std::process::exit(0);
    };
    let Some(cmd) = tree::tree_command(tree_name, dir, name) else {
        eprintln!("haj: {tree_name} に {name} はありません");
        std::process::exit(1);
    };
    match env_report(&cmd) {
        Some(v) => println!("{v}"),
        None => {
            eprintln!(
                "haj: {name} は --haj-env に対応していません ({})",
                cmd.path.display()
            );
            std::process::exit(1);
        }
    }
    std::process::exit(0);
}

/// `haj which <ツリー名> <名前>`(SPEC §9.7)。
fn tree_which(tree_name: &str, dir: &std::path::Path, name: &str) -> ! {
    match tree::tree_command(tree_name, dir, name) {
        Some(cmd) => {
            println!("{}", cmd.path.display());
            std::process::exit(0);
        }
        None => {
            eprintln!("haj: {tree_name} に {name} はありません");
            std::process::exit(1);
        }
    }
}

/// 見つかったコマンド/タスクを exec(2) で実行する(戻らない)。
///
/// exec で自分を置き換える。ラッパープロセスを残さないので、シグナルも
/// 終了コードもサブコマンドのものがそのまま呼び出し元に伝わる。
fn exec_command(cmd: &Command, rest: &[String], deliveries: &[secrets::Delivery]) -> ! {
    // store:// の文脈と tree.* 注入(SPEC §10.7 / §10.8)は「渡す相手のコマンドが
    // 属するツリー」で決まる。インストール済みツリー以外では None。
    let tree_ctx = match &cmd.origin {
        project::Origin::Tree(name) => Some(name.as_str()),
        _ => None,
    };
    let mut proc = prepare_proc(&cmd.path, rest, deliveries, tree_ctx);

    proc.env("HAJ_NAME", &cmd.name);
    match &cmd.root {
        Some(root) => proc.env("HAJ_ROOT", root),
        None => proc.env_remove("HAJ_ROOT"),
    };

    // インストール済みツリー由来なら、インストール名を HAJ_TREE として渡す(SPEC §3.1)。
    // 同じツリーを別名で多重インストールしたとき、ローカル状態(トークン等)の置き場を
    // インスタンスごとに分けるための名前。呼び出しの形(素の探索/名前空間)に依らず、
    // インストール先のパスから決まる同じ値(= Origin::Tree のインストール名)。
    // それ以外の出自では**消す** — 呼び出し元の環境に残った古い値を継がせない。
    match &cmd.origin {
        project::Origin::Tree(name) => proc.env("HAJ_TREE", name),
        _ => proc.env_remove("HAJ_TREE"),
    };

    // いま「どのプロジェクトに対して」操作しているのか。
    //
    // setup や reset は破壊的なので、どのプロジェクトが対象なのかをサブコマンド自身が
    // 名乗れないと事故る。cwd から決まる現在のプロジェクトを渡す
    // (コマンドが属するツリーは HAJ_ROOT。root=false の入れ子では両者は一致しない)。
    apply_project_env(&mut proc);

    let err = proc.exec(); // 成功すれば戻ってこない
    eprintln!("haj: {} を実行できません: {err}", cmd.path.display());

    // exec が ENOENT を返したのにファイル自体は在る、という状況は
    // 「shebang の指すインタプリタが無い」ときに起きる。カーネルは
    // 「実行ファイルが無い」と区別できない形でこれを返してくるので、
    // 素のメッセージだけ見せると原因の見当がつかない(alpine に bash が
    // 無いのに #!/bin/bash と書いてある、など)。ここで補足する。
    if err.kind() == std::io::ErrorKind::NotFound && cmd.path.exists() {
        if let Some(interp) = shebang_interpreter(&cmd.path) {
            eprintln!("  shebang が指すインタプリタが見つかりません: {interp}");
        }
    }
    std::process::exit(126); // 「見つかったが実行できない」
}

/// `haj run [<名前>] [引数...]`(SPEC §9.6)— プロジェクトのタスク。
///
/// 見るのは現在のプロジェクトの `.haj/tasks/<名前>` だけ(1行宣言 task.* は
/// main の展開ループが処理済み)。探索に乗せない・素の名前と重ねない・
/// フォールバックしない — この制約が「haj run x はそのリポジトリのタスク以外に
/// 解釈できない」という保証になる。
fn run_task(args: &[String], deliveries: &[secrets::Delivery]) -> ! {
    if tasks::project_haj().is_none() {
        die("run: プロジェクトの外です (.haj が見つかりません)");
    }
    let Some((name, rest)) = args.split_first() else {
        list_tasks();
    };
    let Some(cmd) = tasks::lookup_file(name) else {
        // 宣言(task.*)がここまで来るのは、展開済みの結果がまた `run <宣言>` だった
        // 場合だけ(展開は1回 — エイリアスの「再帰しない」と同じ規則)。
        if tasks::lookup_decl(name).is_some() {
            eprintln!("haj: タスクの展開は1回だけです: {name} (task.* から task.* へは繋げない)");
        } else if discovery::resolve(name).is_some() || aliases::lookup(name).is_some() {
            eprintln!(
                "haj: {name} はタスクではありません。コマンドとして定義されています: haj {name}"
            );
        } else {
            eprintln!("haj: 未知のタスクです: {name} (haj run で一覧)");
        }
        std::process::exit(127);
    };
    exec_command(&cmd, rest, deliveries)
}

/// `haj run`(引数なし)— タスクの一覧。npm run と同じ振る舞い。
fn list_tasks() -> ! {
    let ts = tasks::list();
    if ts.is_empty() {
        println!("このプロジェクトのタスクはありません。");
        println!(
            "  置き場所: .haj/tasks/<名前> (実行ファイル) / .haj/config の task.<名前> (1行の委譲)"
        );
        std::process::exit(0);
    }
    if let Some(p) = discovery::active_project() {
        println!(" プロジェクト: {}  ({})", p.name, p.dir.display());
    }
    println!("\n タスク (haj run <名前> で実行):");
    let mut cache = DescribeCache::load();
    let width = ts.iter().map(|t| t.name().len()).max().unwrap_or(8);
    for t in &ts {
        println!(
            "   {:width$}  {}",
            t.name(),
            task_summary(&mut cache, t),
            width = width
        );
    }
    cache.save();
    std::process::exit(0);
}

/// タスクの一行説明。宣言は .desc か展開そのもの、ファイルは --haj-describe(キャッシュ経由)。
fn task_summary(cache: &mut DescribeCache, t: &tasks::Task) -> String {
    match t {
        tasks::Task::Decl {
            expansion, desc, ..
        } => match desc {
            Some(d) => d.clone(),
            None => aliases::expansion_summary(expansion),
        },
        tasks::Task::File(cmd) => describe(cache, cmd).unwrap_or_default(),
    }
}

/// `--haj-env` の中継に、コアが知っている**出所**を注記する(SPEC §10.8)。
/// tree設定(tree.<名前>.env / .secret)で決まる鍵に行末コメントが付く。
/// コメントは --env-file で読み飛ばされるので、出力の互換は変わらない。
fn env_report(cmd: &Command) -> Option<String> {
    let out = contract::env_vars(cmd)?;
    match &cmd.origin {
        project::Origin::Tree(name) => Some(store::annotate_env(&out, name)),
        _ => Some(out),
    }
}

/// 複数コマンドの `--haj-env` を `# ==== <名前> ====` の節で連結する(SPEC §9.6 / §9.7)。
/// 応答しないコマンドは黙って飛ばす(何も足せないため)。出力は --env-file に
/// 渡せる形式のまま。1つも応答しなければ None。
fn env_sections(cmds: &[Command]) -> Option<String> {
    let mut out = String::new();
    for c in cmds {
        if let Some(v) = env_report(c) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("# ==== {} ====\n{v}\n", c.name));
        }
    }
    (!out.is_empty()).then_some(out)
}

/// `haj env run [<名前>]`(SPEC §9.6)。名前なしは全タスクの連結。
fn task_env(name: Option<&str>) -> ! {
    let Some(name) = name else {
        let files: Vec<Command> = tasks::list()
            .into_iter()
            .filter_map(|t| match t {
                tasks::Task::File(cmd) => Some(cmd),
                tasks::Task::Decl { .. } => None, // 宣言は委譲であって環境変数を持たない
            })
            .collect();
        match env_sections(&files) {
            Some(v) => print!("{v}"),
            None => {
                eprintln!("haj: --haj-env に対応するタスクがありません");
                std::process::exit(1);
            }
        }
        std::process::exit(0);
    };
    match tasks::lookup(name) {
        Some(tasks::Task::File(cmd)) => match contract::env_vars(&cmd) {
            Some(v) => println!("{v}"),
            None => {
                eprintln!(
                    "haj: {name} は --haj-env に対応していません ({})",
                    cmd.path.display()
                );
                std::process::exit(1);
            }
        },
        Some(tasks::Task::Decl { expansion, .. }) => {
            eprintln!(
                "haj: task.{name} は1行の宣言です (= {expansion})。環境変数は展開先に聞くこと"
            );
            std::process::exit(1);
        }
        None => {
            eprintln!("haj: 未知のタスクです: {name}");
            std::process::exit(1);
        }
    }
    std::process::exit(0);
}

/// SIGPIPE を既定に戻す。依存クレートは増やさない — libc の signal(2) を
/// 直接宣言する(Linux では SIGPIPE=13 / SIG_DFL=0。ビルドターゲットは
/// musl の Linux だけなので、この定数で足りる)。
fn reset_sigpipe() {
    extern "C" {
        fn signal(signum: i32, handler: usize) -> usize;
    }
    unsafe {
        signal(13, 0);
    }
}

/// 先頭の `~` を HOME に展開する。設定ファイル由来のエイリアス展開には
/// シェルがいないので、コアがやらないと alias に `-C ~/...` が書けない。
fn expand_home(path: &str) -> std::path::PathBuf {
    if path == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return std::path::PathBuf::from(home);
        }
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return std::path::PathBuf::from(home).join(rest);
        }
    }
    std::path::PathBuf::from(path)
}

fn die(msg: &str) -> ! {
    eprintln!("haj: {msg}");
    std::process::exit(1);
}

/// シークレット参照の展開(SPEC §10)を適用した子プロセスを組み立てる。
/// 解決に失敗したら本体を実行せずに終了する(fail-fast — 未解決の参照文字列が
/// パスワードとしてそのまま使われる事故を防ぐ)。
/// 規約フック(--haj-describe 等)はこの経路を通らないので展開されない。
/// HAJ_PROJECT / HAJ_PROJECT_DIR を cwd から決めて注入する(SPEC §3.1)。
/// プロジェクトの外では**消す** — 呼び出し元の環境に残った古い値を継がせない。
fn apply_project_env(proc: &mut Proc) {
    match discovery::active_project() {
        Some(p) => {
            proc.env("HAJ_PROJECT", &p.name);
            proc.env("HAJ_PROJECT_DIR", &p.dir);
        }
        None => {
            proc.env_remove("HAJ_PROJECT");
            proc.env_remove("HAJ_PROJECT_DIR");
        }
    }
}

fn prepare_proc(
    path: &std::path::Path,
    args: &[String],
    deliveries: &[secrets::Delivery],
    tree_ctx: Option<&str>,
) -> Proc {
    let mut proc = Proc::new(path);
    proc.args(args);

    // 実行時変数(SPEC §3.1)。全 exec 経路(サブコマンド・タスク・sh 委譲・
    // haj exec)がここを通るので、探索結果に依存しない変数は一括で注入する。
    contract::apply_runtime_env(&mut proc);

    // ツリーごとの設定注入(SPEC §10.8)。注入は `.env`(平文)だけ — `.secret` は
    // 宣言であり、コマンドが haj secret get で引く(§10.9)。**フラグの適用より先** —
    // その変数がシェル環境に無いときだけ注入し、フラグは後から上書きするので、
    // 優先順位は フラグ > シェル環境 > tree設定 > コマンド既定値 になる。
    if let Some(tree) = tree_ctx {
        store::inject_tree_env(&mut proc, tree);
    }

    // シークレットは**人が明示的に渡すものだけ**(SPEC §10)。環境を勝手に走査しない。
    // 書いた順に適用するので、同名の指定は後勝ち。
    for d in deliveries {
        if let Err(e) = d.apply(&mut proc, tree_ctx) {
            eprintln!("haj: {e}");
            std::process::exit(1);
        }
    }

    proc
}

/// `haj exec [--] <プログラム> [引数...]`(SPEC §9.2)。
///
/// 探索を通さず、PATH のコマンド(`/` を含めばそのパス)に**シークレットの注入だけ
/// して**実行する。op run / doppler run が占めている場所。
/// 先頭の `--` は読み飛ばす(`op run --` / `kubectl exec --` の指癖と互換)。
fn exec_external(args: &[String], deliveries: &[secrets::Delivery]) -> ! {
    let args = strip_dashdash(args);
    let Some((prog, prog_args)) = args.split_first() else {
        die("使い方: haj exec [--] <プログラム> [引数...]");
    };
    exec_program(prog, prog_args.to_vec(), deliveries)
}

/// `haj sh '<コマンド>' [引数...]`(SPEC §9.2)— `haj exec sh -c` の省略形。
/// 追加の引数はシェルの位置パラメータ($1...)として渡る。
///
/// `haj sh -- ls -la` のように `--` で始めたときは、以降の語を空白で繋いで
/// 1行にする(ssh 方式)。引用が要る引数を含むなら1つの文字列で書くこと。
fn exec_shell(args: &[String], deliveries: &[secrets::Delivery]) -> ! {
    const USAGE: &str = "使い方: haj sh '<コマンド>' [引数...] / haj sh -- <語...>";

    let (script, rest): (String, &[String]) = if args.first().is_some_and(|a| a == "--") {
        if args.len() < 2 {
            die(USAGE);
        }
        (args[1..].join(" "), &[])
    } else {
        let Some((script, rest)) = args.split_first() else {
            die(USAGE);
        };
        (script.clone(), rest)
    };

    // 自前の `--` で sh のオプション解釈を終わらせる。これが無いと、スクリプトが
    // `-` で始まるときに sh がオプションと誤解し、その次の語($0 用の "haj")を
    // コマンド文字列として実行してしまう。
    // sh -c <script> の直後の引数は $0。$1 から始めたいので "haj" を埋める。
    let mut argv = vec![
        "-c".to_string(),
        "--".to_string(),
        script,
        "haj".to_string(),
    ];
    argv.extend(rest.iter().cloned());
    exec_program("sh", argv, deliveries)
}

/// 先頭の `--` を1つだけ読み飛ばす。
fn strip_dashdash(args: &[String]) -> &[String] {
    match args.first() {
        Some(a) if a == "--" => &args[1..],
        _ => args,
    }
}

fn exec_program(prog: &str, args: Vec<String>, deliveries: &[secrets::Delivery]) -> ! {
    let path = if prog.contains('/') {
        std::path::PathBuf::from(prog)
    } else {
        match discovery::find_in_path(prog) {
            Some(p) => p,
            None => {
                eprintln!("haj: exec: 見つかりません: {prog}");
                std::process::exit(127);
            }
        }
    };

    // haj の外の世界のコマンドにツリー文脈は無い(store:// はエラーになる — §10.7)
    let mut proc = prepare_proc(&path, &args, deliveries, None);

    // haj の外の世界のコマンドに、hajサブコマンドの顔(HAJ_ROOT / HAJ_NAME /
    // HAJ_TREE)はさせない。ただし HAJ_PROJECT は「サブコマンドであること」ではなく
    // 「どこに対して実行しているか」の情報なので渡す — プロジェクト・エイリアスが
    // sh へ委譲したとき(alias.hello = sh -- echo $HAJ_PROJECT)に自分の対象を
    // 名乗れないのは §2.4(素性の可視化)に反する。
    proc.env_remove("HAJ_ROOT")
        .env_remove("HAJ_NAME")
        .env_remove("HAJ_TREE");
    apply_project_env(&mut proc);

    let err = proc.exec(); // 成功すれば戻ってこない
    eprintln!("haj: {} を実行できません: {err}", path.display());
    if err.kind() == std::io::ErrorKind::NotFound && path.exists() {
        if let Some(interp) = shebang_interpreter(&path) {
            eprintln!("  shebang が指すインタプリタが見つかりません: {interp}");
        }
    }
    std::process::exit(126);
}

/// 1行目が `#!` で始まっていれば、そのインタプリタのパスを返す。
fn shebang_interpreter(path: &std::path::Path) -> Option<String> {
    let head = std::fs::read(path).ok()?;
    let line = head.split(|&b| b == b'\n').next()?;
    let line = std::str::from_utf8(line).ok()?;
    let rest = line.strip_prefix("#!")?.trim();
    Some(rest.split_whitespace().next()?.to_string())
}

fn describe(cache: &mut DescribeCache, cmd: &Command) -> Option<String> {
    cache.get_or_insert(contract::stamp(&cmd.path), || contract::describe(cmd))
}

/// `haj which <名前>` / `haj which --all <名前>`
///
/// 同名のコマンドが複数あるとき、どれが勝っていて何が隠れているのかを見せる。
/// 探索順が cwd に依存する以上、これが無いと調べようがない。
fn which(args: &[String]) -> ! {
    let all = args.iter().any(|a| a == "--all" || a == "-a");

    // タスク(SPEC §9.6): haj which run <名前> — 効いている定義(宣言かファイル)を見せる
    let non_flags: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    if non_flags.first().map(|s| s.as_str()) == Some("run") {
        if let Some(tname) = non_flags.get(1) {
            task_which(tname, all);
        }
    }

    // ツリー名前空間(SPEC §9.7): haj which <ツリー名> [<名前>]
    if let Some(first) = non_flags.first() {
        if let Some(dir) = tree::find(first) {
            match non_flags.get(1) {
                Some(cname) => tree_which(first, &dir, cname),
                None => {
                    println!(
                        "{}  [ツリー {}]  (haj which {} <名前> でコマンドのパス)",
                        dir.display(),
                        first,
                        first
                    );
                    std::process::exit(0);
                }
            }
        }
    }

    let Some(target) = args.iter().find(|a| !a.starts_with('-')) else {
        eprintln!("haj: 使い方: haj which [--all] <コマンド名> / haj which run <タスク>");
        std::process::exit(1);
    };

    // エイリアスなら展開と出自を見せる(素性の可視化。エイリアスは探索より優先)
    let alias = aliases::lookup(target);
    if let Some(a) = &alias {
        if !all {
            println!("alias.{target} = {}  {}", a.expansion, a.origin.label());
            std::process::exit(0);
        }
        println!("* alias.{target} = {}  {}", a.expansion, a.origin.label());
    }

    let cands = discovery::candidates(target);
    if cands.is_empty() && alias.is_none() {
        eprintln!("haj: 未知のコマンドです: {target}");
        std::process::exit(1);
    }

    if !all {
        if let Some(c) = cands.first() {
            println!("{}", c.path.display());
        }
        std::process::exit(0);
    }

    for (i, c) in cands.iter().enumerate() {
        let mark = if i == 0 && alias.is_none() { "*" } else { " " };
        println!("{mark} {} {}", c.path.display(), c.origin.label());
    }
    if cands.len() + usize::from(alias.is_some()) > 1 {
        println!("\n(* が実行されるもの。他は隠れている)");
    }
    std::process::exit(0);
}

/// `haj which run <名前>`(SPEC §9.6)— タスクの効いている定義を見せる。
/// 同名が宣言(task.*)とファイル(tasks/)の両方にあれば宣言が勝つ。
fn task_which(name: &str, all: bool) -> ! {
    let decl = tasks::lookup_decl(name);
    let file = tasks::lookup_file(name);
    if decl.is_none() && file.is_none() {
        eprintln!("haj: 未知のタスクです: {name}");
        std::process::exit(1);
    }

    if let Some(tasks::Task::Decl { expansion, .. }) = &decl {
        if !all {
            println!("task.{name} = {expansion}");
            std::process::exit(0);
        }
        println!("* task.{name} = {expansion}");
    }
    if let Some(cmd) = &file {
        if !all {
            println!("{}", cmd.path.display());
            std::process::exit(0);
        }
        let mark = if decl.is_none() { "*" } else { " " };
        println!("{mark} {} {}", cmd.path.display(), cmd.origin.label());
    }
    if all && decl.is_some() && file.is_some() {
        println!("\n(* が実行されるもの。他は隠れている)");
    }
    std::process::exit(0);
}

/// `haj help` / `haj help <名前>`
///
/// コマンド一覧は --haj-describe を全コマンドに聞いて自動生成する。
/// 前後の固定文だけ help.header / help.footer から読む。コマンドを足しても
/// ヘルプを書き足す必要がない、というのがこの設計の主眼。
fn print_help(args: &[String]) {
    if let Some(topic) = args.first().map(String::as_str) {
        // タスクの使い方(SPEC §9.6): haj help run <名前>
        if topic == "run" {
            if let Some(tname) = args.get(1) {
                print_task_help(tname);
                return;
            }
        }
        // 組み込みは探索の対象ではないので、先に見る
        if let Some(h) = builtin::long_help(topic) {
            println!("{h}");
            return;
        }
        // ツリー名前空間(§9.7): haj help <ツリー名> [<名前>]
        if let Some(dir) = tree::find(topic) {
            let Some(cname) = args.get(1) else {
                list_tree_commands(topic, &dir);
            };
            let Some(cmd) = tree::tree_command(topic, &dir, cname) else {
                eprintln!("haj: {topic} に {cname} はありません");
                std::process::exit(1);
            };
            match contract::long_help(&cmd) {
                Some(h) => println!("{h}"),
                None => println!(
                    "{} には使い方の説明がありません ({})",
                    cmd.name,
                    cmd.path.display()
                ),
            }
            return;
        }
        let Some(cmd) = discovery::resolve(topic) else {
            eprintln!("haj: 未知のコマンドです: {topic}");
            std::process::exit(1);
        };
        match contract::long_help(&cmd) {
            Some(h) => println!("{h}"),
            None => println!(
                "{} には使い方の説明がありません ({})",
                cmd.name,
                cmd.path.display()
            ),
        }
        return;
    }

    if let Some(header) = contract::fragment("header") {
        print!("{header}");
    }

    // いまどのプロジェクトの中にいるのかを最初に言う。
    // 同名のコマンド(setup など)がプロジェクトごとに違う挙動をする以上、
    // 「どのプロジェクトの haj を見ているのか」が分からないままでは危ない。
    if let Some(p) = discovery::active_project() {
        println!("\n プロジェクト: {}  ({})", p.name, p.dir.display());
    }

    // docs の入口を冒頭で示す(SPEC §5)。「haj自身」の一覧の中に埋もれると、
    // ドキュメントがあること自体に気づけない。
    println!("\n ドキュメント: haj docs  (一覧から選んで読む。fzfがあれば選択UI)");

    let mut cache = DescribeCache::load();
    let cmds = discovery::list();

    // 名前の桁幅は、探索で見つかったものと組み込みで揃える(2つの表がズレると読みにくい)
    let width = cmds
        .iter()
        .map(|c| c.name.len())
        .chain(builtin::ALL.iter().map(|b| b.name.len()))
        .max()
        .unwrap_or(8);

    if cmds.is_empty() {
        println!("\nこのプロジェクトのコマンドはありません。");
        println!("  探索先: {}", dirs_hint());
    } else {
        println!("\n hajコマンド (haj help <名前> で詳細):");
        let dwidth = cmds
            .iter()
            .map(|c| describe(&mut cache, c).unwrap_or_default().chars().count())
            .max()
            .unwrap_or(0)
            .min(48);
        for cmd in &cmds {
            let d = describe(&mut cache, cmd).unwrap_or_default();
            // 出自を右端に出す。同名でどれが効いているか、一覧の時点で見えるように。
            println!(
                "   {:width$}  {:dwidth$}  {}",
                cmd.name,
                d,
                cmd.origin.label(),
                width = width,
                dwidth = dwidth,
            );
        }
    }
    // プロジェクトのタスク(SPEC §9.6)。呼べる名前である以上、一覧から漏らさない。
    // 出自は常にこのプロジェクトなので、ラベルは節の見出しが兼ねる。
    let ts = tasks::list();
    if !ts.is_empty() {
        println!("\n プロジェクトのタスク (haj run <名前> で実行):");
        let twidth = ts
            .iter()
            .map(|t| t.name().len())
            .max()
            .unwrap_or(0)
            .max(width);
        for t in &ts {
            println!(
                "   {:twidth$}  {}",
                t.name(),
                task_summary(&mut cache, t),
                twidth = twidth
            );
        }
    }
    // 名前空間のツリー(SPEC §9.7)。素の一覧から消えている分、入口をここに出す。
    // コマンドを並べず1行に畳む(一覧の長さを膨らませない)。
    let ns_trees = tree::namespaced();
    if !ns_trees.is_empty() {
        println!("\n ツリー名前空間 (haj <ツリー名> <名前> で実行):");
        let nwidth = ns_trees
            .iter()
            .map(|(n, _)| n.len())
            .max()
            .unwrap_or(0)
            .max(width);
        for (n, dir) in &ns_trees {
            let count = tree::tree_commands(n, dir).len();
            println!(
                "   {:nwidth$}  {count} コマンド (haj {n} で一覧)",
                n,
                nwidth = nwidth
            );
        }
    }
    cache.save();

    // 組み込みはどこにいても使える。探索されないからといって一覧から漏らすと、
    // 「haj help の一覧が実態と一致する」という約束が嘘になる。
    // ただしプロジェクトのコマンドとは性質が違うので、節を分けて出す。
    println!("\n haj自身 (どのプロジェクトでも使える):");
    for b in builtin::ALL {
        println!("   {:width$}  {}", b.name, b.describe, width = width);
    }

    // エイリアス(SPEC §2.7)。呼べる名前である以上、一覧から漏らさない。
    // 出自(プロジェクト / 設定ファイル)も右端に出す — コマンドの表と同じ規律。
    let aliases = aliases::list();
    if !aliases.is_empty() {
        println!("\n エイリアス (設定ファイルまたは .haj/config の alias.*):");
        let awidth = aliases.iter().map(|a| a.name.len()).max().unwrap_or(0);
        let dwidth = aliases
            .iter()
            .map(|a| a.summary().chars().count())
            .max()
            .unwrap_or(0);
        for a in &aliases {
            println!(
                "   {:awidth$}  {:dwidth$}  {}",
                a.name,
                a.summary(),
                a.origin.label(),
                awidth = awidth,
                dwidth = dwidth,
            );
        }
    }

    // グローバルフラグ。コマンドと違って探索でも組み込み表でもないので、
    // ここに載せないとどこにも出ない(一覧が実態と一致する、という約束の一部)。
    println!("\n グローバルフラグ (コマンド名の前に書く):");
    println!("   -C <ディレクトリ>                   そのディレクトリを起点に実行する (gitと同じ。複数可)");
    println!("   --secret <名前>=<値>              参照を展開して環境変数で渡す");
    println!("   --env-file <ファイル>              key = value を読み、値を展開して渡す");
    println!("   --secret-file <名前|パス>=<参照|テンプレート>  中身をファイルにして渡す (0600)");
    println!("   (シークレット参照の詳細は haj help secrets)");

    if let Some(footer) = contract::fragment("footer") {
        print!("{footer}");
    }
    let _ = std::io::stdout().flush();
}

/// `haj help run <名前>`(SPEC §9.6)— タスクの使い方。
/// 宣言は展開そのものが使い方(which と同じ見せ方)、ファイルは --haj-help に聞く。
fn print_task_help(name: &str) {
    match tasks::lookup(name) {
        Some(tasks::Task::Decl {
            expansion, desc, ..
        }) => {
            if let Some(d) = desc {
                println!("{d}");
            }
            println!("task.{name} = {expansion}");
        }
        Some(tasks::Task::File(cmd)) => match contract::long_help(&cmd) {
            Some(h) => println!("{h}"),
            None => println!(
                "{} には使い方の説明がありません ({})",
                cmd.name,
                cmd.path.display()
            ),
        },
        None => {
            eprintln!("haj: 未知のタスクです: {name}");
            std::process::exit(1);
        }
    }
}

fn print_usage() {
    eprintln!("{USAGE_FLAGS}\n");
    let mut cache = DescribeCache::load();
    let cmds = discovery::list();
    if cmds.is_empty() {
        eprintln!("コマンドが1つも見つかりません。");
        eprintln!("  探索先: {}", dirs_hint());
    } else {
        eprintln!("使えるコマンド:");
        let width = cmds.iter().map(|c| c.name.len()).max().unwrap_or(0).max(8);
        for cmd in &cmds {
            let d = describe(&mut cache, cmd).unwrap_or_default();
            eprintln!(
                "  {:width$}  {}  {}",
                cmd.name,
                d,
                cmd.origin.label(),
                width = width
            );
        }
    }
    cache.save();
    eprintln!("\n  haj help          全体の使い方");
    eprintln!("  haj help <名前>    個別の使い方");
}

fn dirs_hint() -> String {
    let dirs = discovery::command_dirs();
    if dirs.is_empty() {
        format!(
            "(該当なし) .haj/commands / ~/.config/haj/commands / {}",
            discovery::DEFAULT_COMMAND_PATH
        )
    } else {
        dirs.iter()
            .map(|d| d.path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// 補完プロトコル(シェル補完スクリプトから呼ばれる)。
///
///   haj __complete                  → "名前\t説明" を列挙
///   haj __complete <名前> [語...]     → そのコマンドの --haj-complete へ転送
///
/// これによりシェル補完スクリプトは候補を一切持たない。コマンドを足しても
/// 補完を書き足す必要がない。
fn complete(args: &[String]) {
    // グローバルフラグ(SPEC §3.2)を本体と同じ規則で読み飛ばす。-C は実際に移動する
    // (移動先のコマンド名を補完するため)。値が未入力のフラグで終わっている場合は
    // 候補を作れない(値の補完はシェル側がファイル/ディレクトリ補完で行う)。
    let mut idx = 0;
    while idx < args.len() {
        match args[idx].as_str() {
            "-C" => {
                let Some(dir) = args.get(idx + 1) else { return };
                let _ = std::env::set_current_dir(expand_home(dir));
                idx += 2;
            }
            "--secret" | "--env-file" | "--secret-file" => {
                if args.len() <= idx + 1 {
                    return;
                }
                idx += 2;
            }
            _ => break,
        }
    }
    let args = &args[idx..];

    let Some((name, words)) = args.split_first() else {
        complete_names();
        return;
    };

    // エイリアスなら展開して、実効コマンドの補完に回す(SPEC §6)。
    // 展開しないと `haj oci <TAB>` のようなエイリアスで補完が死ぬ。
    let (mut name, mut words): (String, Vec<String>) = match aliases::lookup(name) {
        Some(a) => match split_expansion(&a.expansion, words) {
            Some(nw) => nw,
            None => {
                // フラグだけのエイリアス(-C など)。移動先のコマンド一覧を出す。
                complete_names();
                return;
            }
        },
        None => (name.to_string(), words.to_vec()),
    };

    // タスク(SPEC §9.6): `haj run <TAB>` はタスク一覧、`haj run <名前> <TAB>` は
    // そのタスクの --haj-complete へ転送。1行宣言(task.*)はエイリアスと同じく
    // 展開してから扱う(exec に解決されれば下の @delegate に落ちる)。
    if name == "run" {
        let Some((tname, twords)) = words.split_first().map(|(t, w)| (t.clone(), w.to_vec()))
        else {
            complete_task_names();
            return;
        };
        match tasks::lookup(&tname) {
            Some(tasks::Task::File(cmd)) => {
                for c in contract::complete(&cmd, &twords) {
                    println!("{c}");
                }
                return;
            }
            Some(tasks::Task::Decl { expansion, .. }) => {
                match split_expansion(&expansion, &twords) {
                    Some((n, w)) => {
                        name = n;
                        words = w;
                    }
                    None => return, // フラグだけの宣言。候補は作れない
                }
            }
            None => return, // 未知のタスク。候補なし(補完中に赤い文字を出さない)
        }
    }
    let words: &[String] = &words;

    // exec / sh は haj の外のコマンドを走らせる。候補は haj には作れないので、
    // **そのコマンド自身の補完に委譲する**ようシェルへ指示を返す(SPEC §6)。
    if name == "exec" {
        let argv = strip_dashdash(words);
        if argv.is_empty() {
            return;
        }
        println!("@delegate\t{}", argv.join("\t"));
        return;
    }
    if name == "sh" {
        return; // シェルの1行に候補は作れない
    }

    // 組み込みは探索の対象ではないので、先に見る
    if builtin::find(&name).is_some() {
        for c in builtin::complete(&name, words) {
            println!("{c}");
        }
        return;
    }

    // ツリー名前空間(§9.7): haj <ツリー名> <TAB> は一覧、以降は転送。
    if let Some(dir) = tree::find(&name) {
        match words.split_first() {
            None => {
                let mut cache = DescribeCache::load();
                for c in tree::tree_commands(&name, &dir) {
                    let d = describe(&mut cache, &c).unwrap_or_default();
                    println!("{}\t{}", c.name, d);
                }
                cache.save();
            }
            Some((cname, cwords)) => {
                if let Some(cmd) = tree::tree_command(&name, &dir, cname) {
                    for c in contract::complete(&cmd, cwords) {
                        println!("{c}");
                    }
                }
            }
        }
        return;
    }

    // 未知のコマンドなら候補なし。エラーにはしない(補完中に赤い文字を出さない)。
    let Some(cmd) = discovery::resolve(&name) else {
        return;
    };
    for c in contract::complete(&cmd, words) {
        println!("{c}");
    }
}

/// 展開文字列を argv に割り、先頭のグローバルフラグを補完用に読み飛ばす
/// (-C は実際に移動する — 移動先のコマンドを補完するため)。エイリアスと
/// タスク宣言(SPEC §9.6)で共用。フラグの後に名前が無ければ None。
fn split_expansion(expansion: &str, extra: &[String]) -> Option<(String, Vec<String>)> {
    let argv: Vec<String> = expansion.split_whitespace().map(str::to_string).collect();
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "-C" => {
                if let Some(dir) = argv.get(i + 1) {
                    let _ = std::env::set_current_dir(expand_home(dir));
                }
                i += 2;
            }
            "--secret" | "--env-file" | "--secret-file" => i += 2,
            _ => break,
        }
    }
    let mut rest: Vec<String> = argv[i.min(argv.len())..].to_vec();
    if rest.is_empty() {
        return None;
    }
    let n = rest.remove(0);
    rest.extend(extra.iter().cloned());
    Some((n, rest))
}

/// `haj __complete run` — タスクの一覧を "名前\t説明" で出す(SPEC §6)。
fn complete_task_names() {
    let mut cache = DescribeCache::load();
    for t in tasks::list() {
        println!("{}\t{}", t.name(), task_summary(&mut cache, &t));
    }
    cache.save();
}

/// 打てる名前を全部出す(探索で見つかるもの + 組み込み + エイリアス)。
fn complete_names() {
    let mut cache = DescribeCache::load();
    let mut rows: Vec<(String, String)> = discovery::list()
        .into_iter()
        .map(|cmd| {
            let d = describe(&mut cache, &cmd).unwrap_or_default();
            (cmd.name, d)
        })
        .collect();
    cache.save();
    // 組み込みも補完に出す。どこにいても打てるのだから、TABで出ないのはおかしい。
    rows.extend(
        builtin::ALL
            .iter()
            .map(|b| (b.name.to_string(), b.describe.to_string())),
    );
    // エイリアスも呼べる名前(SPEC §2.7)
    rows.extend(aliases::list().into_iter().map(|a| {
        let d = a.summary();
        (a.name, d)
    }));
    // ツリー名も呼べる名前(SPEC §9.7 — 名前空間の入口)
    rows.extend(tree::installed().into_iter().map(|(n, _)| {
        let d = format!("ツリーのコマンド (haj {n} で一覧)");
        (n, d)
    }));
    rows.sort();
    rows.dedup_by(|a, b| a.0 == b.0);
    for (name, desc) in rows {
        println!("{name}\t{desc}");
    }
}
