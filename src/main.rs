//! haj — サブコマンドを探索して実行する、薄いルーター。
//!
//! コアはディスパッチ表を持たない。`haj mig` は「どこかにある実行可能ファイル
//! `mig`」を探して exec するだけ。プロジェクトごとに異なるサブコマンドの
//! サブセットは、リポジトリに `.haj/commands/` を置くことで自然に成立する。
//!
//! 仕様は SPEC.md を参照。

mod builtin;
mod cache;
mod config;
mod contract;
mod discovery;
mod project;
mod selfupgrade;

use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::Command as Proc;

use cache::DescribeCache;
use discovery::Command;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (name, rest) = match args.split_first() {
        None => {
            // 素の `haj` はヘルプ。何も分からない状態で来た人に一覧を見せるのが親切。
            print_help(None);
            std::process::exit(0);
        }
        Some((n, r)) => (n.as_str(), r),
    };

    match name {
        "-h" | "--help" | "help" => {
            print_help(rest.first().map(String::as_str));
            std::process::exit(0);
        }
        "-V" | "--version" => {
            println!("haj {VERSION}");
            std::process::exit(0);
        }
        "config" => {
            config::show();
            std::process::exit(0);
        }
        "selfupgrade" => selfupgrade::run(rest),
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
    let mut proc = Proc::new(&cmd.path);
    proc.args(rest).env("HAJ_NAME", &cmd.name);
    match &cmd.root {
        Some(root) => proc.env("HAJ_ROOT", root),
        None => proc.env_remove("HAJ_ROOT"),
    };

    // いま「どのプロジェクトに対して」操作しているのか。
    //
    // setup や reset は破壊的なので、どのプロジェクトが対象なのかをサブコマンド自身が
    // 名乗れないと事故る。cwd から決まる現在のプロジェクトを渡す
    // (コマンドが属するツリーは HAJ_ROOT。root=false の入れ子では両者は一致しない)。
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

    let cands = discovery::candidates(target);
    if cands.is_empty() {
        eprintln!("haj: 未知のコマンドです: {target}");
        std::process::exit(1);
    }

    if !all {
        println!("{}", cands[0].path.display());
        std::process::exit(0);
    }

    for (i, c) in cands.iter().enumerate() {
        let mark = if i == 0 { "*" } else { " " };
        println!("{mark} {} {}", c.path.display(), c.origin.label());
    }
    if cands.len() > 1 {
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

    if let Some(footer) = contract::fragment("footer") {
        print!("{footer}");
    }
    let _ = std::io::stdout().flush();
}

fn print_usage() {
    eprintln!("使い方: haj <コマンド> [引数...]\n");
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
            "(該当なし) .haj/commands / ~/.haj/commands / {}",
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
        rows.sort();
        for (name, desc) in rows {
            println!("{name}\t{desc}");
        }
        return;
    };

    // 組み込みは探索の対象ではないので、先に見る
    if builtin::find(name).is_some() {
        for c in builtin::complete(name, words) {
            println!("{c}");
        }
        return;
    }

    // 未知のコマンドなら候補なし。エラーにはしない(補完中に赤い文字を出さない)。
    let Some(cmd) = discovery::resolve(name) else {
        return;
    };
    for c in contract::complete(&cmd, words) {
        println!("{c}");
    }
}
