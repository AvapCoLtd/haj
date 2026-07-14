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
mod secrets;
mod selfupgrade;
mod tree;

use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::Command as Proc;

use cache::DescribeCache;
use discovery::Command;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE_FLAGS: &str = "使い方: haj [-C <ディレクトリ>] [--secret <名前>=<値>]... [--env-file <ファイル>]... [--secret-file <名前|パス>=<参照|テンプレート>]... <コマンド> [引数...]";

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    let mut deliveries: Vec<secrets::Delivery> = Vec::new();
    let mut alias_expanded = false;

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
                print_help(None);
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
                break (n.to_string(), r.to_vec());
            }
        }
    };
    let name = name.as_str();
    let rest: &[String] = &rest;

    // 受け渡しフラグは「本体を実行する」ときと、その dry-run のときにだけ意味がある。
    // それ以外の組み込みに続けて書かれたら使い方の誤り(SPEC §10.2)。
    if !deliveries.is_empty()
        && !matches!(name, "exec" | "sh" | "secrets")
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
        "-h" | "--help" | "help" => {
            print_help(rest.first().map(String::as_str));
            std::process::exit(0);
        }
        "-V" | "--version" => {
            println!("haj {VERSION}");
            std::process::exit(0);
        }
        "config" => {
            // --init は設定ファイルの雛形を標準出力へ(全行コメント)。
            // そのままリダイレクトすれば初期化になる。SPEC §8.2。
            if rest.first().map(String::as_str) == Some("--init") {
                config::template();
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
        // 何が展開されるのかを、金庫に触らずに確かめる。SPEC.md §10.6。
        "secrets" => secrets::run(rest, &deliveries),
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

    let Some(cmd) = discovery::resolve(name) else {
        eprintln!("haj: 未知のコマンドです: {name}\n");
        print_usage();
        std::process::exit(127); // シェルの「command not found」に合わせる
    };

    // exec で自分を置き換える。ラッパープロセスを残さないので、シグナルも
    // 終了コードもサブコマンドのものがそのまま呼び出し元に伝わる。
    let mut proc = prepare_proc(&cmd.path, rest, &deliveries);

    proc.env("HAJ_NAME", &cmd.name);
    match &cmd.root {
        Some(root) => proc.env("HAJ_ROOT", root),
        None => proc.env_remove("HAJ_ROOT"),
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

fn prepare_proc(path: &std::path::Path, args: &[String], deliveries: &[secrets::Delivery]) -> Proc {
    let mut proc = Proc::new(path);
    proc.args(args);

    // シークレットは**人が明示的に渡すものだけ**(SPEC §10)。環境を勝手に走査しない。
    // 書いた順に適用するので、同名の指定は後勝ち。
    for d in deliveries {
        if let Err(e) = d.apply(&mut proc) {
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

    let mut proc = prepare_proc(&path, &args, deliveries);

    // haj の外の世界のコマンドに、hajサブコマンドの顔(HAJ_ROOT / HAJ_NAME)は
    // させない。ただし HAJ_PROJECT は「サブコマンドであること」ではなく
    // 「どこに対して実行しているか」の情報なので渡す — プロジェクト・エイリアスが
    // sh へ委譲したとき(alias.hello = sh -- echo $HAJ_PROJECT)に自分の対象を
    // 名乗れないのは §2.4(素性の可視化)に反する。
    proc.env_remove("HAJ_ROOT").env_remove("HAJ_NAME");
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
    let Some(target) = args.iter().find(|a| !a.starts_with('-')) else {
        eprintln!("haj: 使い方: haj which [--all] <コマンド名>");
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

/// `haj help` / `haj help <名前>`
///
/// コマンド一覧は --haj-describe を全コマンドに聞いて自動生成する。
/// 前後の固定文だけ help.header / help.footer から読む。コマンドを足しても
/// ヘルプを書き足す必要がない、というのがこの設計の主眼。
fn print_help(topic: Option<&str>) {
    if let Some(topic) = topic {
        // 組み込みは探索の対象ではないので、先に見る
        if let Some(h) = builtin::long_help(topic) {
            println!("{h}");
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
    let Some((name, words)) = args.split_first() else {
        complete_names();
        return;
    };

    // エイリアスなら展開して、実効コマンドの補完に回す(SPEC §6)。
    // 展開しないと `haj oci <TAB>` のようなエイリアスで補完が死ぬ。
    let (name, words): (String, Vec<String>) = match aliases::lookup(name) {
        Some(a) => {
            let argv: Vec<String> = a.expansion.split_whitespace().map(str::to_string).collect();
            let mut i = 0;
            while i < argv.len() {
                match argv[i].as_str() {
                    // -C は実際に移動する。移動先のコマンドを補完するため
                    // (`alias.pj = -C ~/repos/x` の `haj pj <TAB>`)。
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
                // フラグだけのエイリアス(-C など)。移動先のコマンド一覧を出す。
                complete_names();
                return;
            }
            let n = rest.remove(0);
            rest.extend(words.iter().cloned());
            (n, rest)
        }
        None => (name.to_string(), words.to_vec()),
    };
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

    // 未知のコマンドなら候補なし。エラーにはしない(補完中に赤い文字を出さない)。
    let Some(cmd) = discovery::resolve(&name) else {
        return;
    };
    for c in contract::complete(&cmd, words) {
        println!("{c}");
    }
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
    rows.sort();
    for (name, desc) in rows {
        println!("{name}\t{desc}");
    }
}
