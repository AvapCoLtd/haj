//! 共有ツリーの配布(SPEC §9.5)。
//!
//! `haj tree install <URL>` は git リポジトリを $XDG_DATA_HOME/haj/trees/<名前> に
//! clone するだけ。入れたツリーが**探索の対象になるだけ**で、パッケージマネージャは
//! 作らない — 探索と exec というコアの原理は変わらない。
//!
//! 状態ファイルは持たない。**git のリポジトリ自体が状態**である(URL は remote、
//! 版は HEAD)。git は op / bao と同じく CLI へ委譲する(依存クレートゼロを維持)。

use std::path::{Path, PathBuf};
use std::process::Command as Proc;

const USAGE: &str = "\
使い方: haj tree install <gitのURL>[@<ref>] [--name <名前>]
        haj tree update [<名前>]
        haj tree list
        haj tree remove <名前>";

/// インストール先: `$XDG_DATA_HOME/haj/trees`(既定 `~/.local/share/haj/trees`)。
/// 設定でも環境変数でもなくデータなので、XDG data に置く。
pub fn trees_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
        .map(|base| base.join("haj").join("trees"))
}

/// ツリーの根: `<dir>/.haj` があればそれ、無ければ `<dir>` 自体(SPEC §9.5)。
/// これで「配布専用リポジトリ」と「.haj を持つ普通の haj プロジェクト」の
/// どちらもそのまま入れられる。
pub fn tree_root(dir: &Path) -> PathBuf {
    let dot = dir.join(".haj");
    if dot.is_dir() {
        dot
    } else {
        dir.to_path_buf()
    }
}

/// インストール済みツリーを名前順に返す(名前 = ディレクトリ名)。
pub fn installed() -> Vec<(String, PathBuf)> {
    let Some(base) = trees_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&base) else {
        return Vec::new();
    };
    let mut v: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok().map(|n| (n, e.path())))
        .filter(|(n, _)| !n.starts_with('.'))
        .collect();
    v.sort();
    v
}

pub fn run(args: &[String]) -> ! {
    match args.split_first().map(|(a, r)| (a.as_str(), r)) {
        None | Some(("list", _)) => list(),
        Some(("install", r)) => install(r),
        Some(("update", r)) => update(r),
        Some(("remove", r)) => remove(r),
        Some((other, _)) => die(&format!("未知のサブコマンドです: {other}\n{USAGE}")),
    }
}

fn install(args: &[String]) -> ! {
    let mut name_flag: Option<String> = None;
    let mut url_arg: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--name" {
            let Some(n) = it.next() else {
                die(&format!("--name には値が要ります\n{USAGE}"));
            };
            name_flag = Some(n.clone());
        } else if url_arg.is_none() {
            url_arg = Some(a.clone());
        } else {
            die(&format!("引数が多すぎます: {a}\n{USAGE}"));
        }
    }
    let Some(url_arg) = url_arg else {
        die(USAGE);
    };
    let (url, reference) = split_ref(&url_arg);

    let Some(base) = trees_dir() else {
        die("HOME が分かりません");
    };
    if let Err(e) = std::fs::create_dir_all(&base) {
        die(&format!("{} を作れません: {e}", base.display()));
    }

    // 一時ディレクトリに clone してから改名する。名前はツリーの config を読まないと
    // 決められない(--name > config の name > リポジトリ名)し、検証に失敗したものを
    // 正規の名前で残したくない。
    let tmp = base.join(format!(".installing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);

    let mut clone_args: Vec<&str> = vec!["clone", "--quiet"];
    if let Some(r) = &reference {
        clone_args.extend(["--branch", r]);
    }
    let tmp_s = tmp.display().to_string();
    clone_args.push(&url);
    clone_args.push(&tmp_s);
    if let Err(e) = git(None, &clone_args) {
        let _ = std::fs::remove_dir_all(&tmp);
        die(&format!("clone できません: {url}\n{e}"));
    }

    // ツリーとして成立しているか。ゴミを黙って入れない。
    // commands/ 必須にはしない — docs/ だけのツリーは §9.3 で正当だし、
    // config だけのツリー(エイリアス集の配布)も正当。
    let root = tree_root(&tmp);
    if !root.join("commands").is_dir()
        && !root.join("docs").is_dir()
        && !root.join("config").is_file()
    {
        let _ = std::fs::remove_dir_all(&tmp);
        die(&format!(
            "これはツリーではありません: {url}\n  commands/ も docs/ も config も(.haj/ の下にも)ありません"
        ));
    }

    // 名前の決定順: --name > ツリーの config の name > リポジトリ名(SPEC §9.5)
    let name = name_flag
        .or_else(|| config_name(&root))
        .unwrap_or_else(|| repo_basename(&url));
    if name.is_empty() || name.starts_with('.') || name.contains('/') {
        let _ = std::fs::remove_dir_all(&tmp);
        die(&format!("ツリー名にできません: {name}"));
    }

    let dest = base.join(&name);
    if dest.exists() {
        let _ = std::fs::remove_dir_all(&tmp);
        die(&format!(
            "既に入っています: {name}\n  更新するなら: haj tree update {name}"
        ));
    }
    if let Err(e) = std::fs::rename(&tmp, &dest) {
        let _ = std::fs::remove_dir_all(&tmp);
        die(&format!("{} に置けません: {e}", dest.display()));
    }

    let n = command_count(&dest);
    println!("インストールしました: {name} ({n} コマンド)");
    println!("  {}", dest.display());
    println!("  一覧に [{name}] として出ます (haj help で確認)");
    std::process::exit(0);
}

fn update(args: &[String]) -> ! {
    let targets: Vec<(String, PathBuf)> = match args.first() {
        Some(name) => {
            let Some(t) = installed().into_iter().find(|(n, _)| n == name) else {
                die(&format!("入っていません: {name}\n  一覧: haj tree list"));
            };
            vec![t]
        }
        None => installed(),
    };
    if targets.is_empty() {
        println!("ツリーは入っていません (haj tree install <URL>)");
        std::process::exit(0);
    }

    let mut failed = false;
    for (name, dir) in targets {
        let old = git(Some(&dir), &["rev-parse", "HEAD"]).unwrap_or_default();
        // ff-only: ローカルに手を入れたツリーを黙って巻き戻さない
        if let Err(e) = git(Some(&dir), &["pull", "--ff-only", "--quiet"]) {
            eprintln!("haj: {name}: 更新できません\n{e}");
            failed = true;
            continue;
        }
        let new = git(Some(&dir), &["rev-parse", "HEAD"]).unwrap_or_default();
        if old == new {
            println!("{name}: 最新です ({})", short(&new));
            continue;
        }
        // 何が変わったのかを黙って入れ替えない(素性の可視化)
        println!("{name}: {} → {}", short(&old), short(&new));
        if let Ok(log) = git(Some(&dir), &["log", "--oneline", &format!("{old}..{new}")]) {
            for line in log.lines() {
                println!("  {line}");
            }
        }
    }
    std::process::exit(if failed { 1 } else { 0 });
}

fn list() -> ! {
    let trees = installed();
    if trees.is_empty() {
        println!("ツリーは入っていません (haj tree install <URL>)");
        std::process::exit(0);
    }
    let width = trees.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
    for (name, dir) in trees {
        let url = git(Some(&dir), &["remote", "get-url", "origin"]).unwrap_or_default();
        let head = git(Some(&dir), &["rev-parse", "--short", "HEAD"]).unwrap_or_default();
        let n = command_count(&dir);
        println!("{name:width$}  {head:8}  {n:3} コマンド  {url}");
    }
    std::process::exit(0);
}

fn remove(args: &[String]) -> ! {
    let Some(name) = args.first() else {
        die("使い方: haj tree remove <名前>\n  一覧: haj tree list");
    };
    let Some((_, dir)) = installed().into_iter().find(|(n, _)| n == name) else {
        die(&format!("入っていません: {name}\n  一覧: haj tree list"));
    };
    if let Err(e) = std::fs::remove_dir_all(&dir) {
        die(&format!("{} を消せません: {e}", dir.display()));
    }
    println!("消しました: {name}");
    std::process::exit(0);
}

/// `<URL>@<ref>` を分ける。`@` 以降に `/` や `:` が含まれるならそれは ref ではなく
/// URL の一部(`git@host:...` の形)。
fn split_ref(arg: &str) -> (String, Option<String>) {
    if let Some(pos) = arg.rfind('@') {
        let after = &arg[pos + 1..];
        if !after.is_empty() && !after.contains('/') && !after.contains(':') {
            return (arg[..pos].to_string(), Some(after.to_string()));
        }
    }
    (arg.to_string(), None)
}

/// URL からリポジトリ名を取る(末尾の `.git` は落とす)。
fn repo_basename(url: &str) -> String {
    let trimmed = url.trim_end_matches('/');
    let base = trimmed.rsplit(['/', ':']).next().unwrap_or(trimmed);
    base.strip_suffix(".git").unwrap_or(base).to_string()
}

/// ツリーの config の `name`(SPEC §9.5。効く鍵はプロジェクト config と同じ発想)。
fn config_name(root: &Path) -> Option<String> {
    let s = std::fs::read_to_string(root.join("config")).ok()?;
    crate::config::parse_kv(&s)
        .get("name")
        .filter(|v| !v.is_empty())
        .cloned()
}

fn command_count(dir: &Path) -> usize {
    let root = tree_root(dir);
    std::fs::read_dir(root.join("commands"))
        .map(|entries| entries.flatten().count())
        .unwrap_or(0)
}

fn short(rev: &str) -> &str {
    if rev.len() >= 8 {
        &rev[..8]
    } else {
        rev
    }
}

/// git を叩く。失敗したら stderr を返す。git 自体が無いことも失敗のうち。
fn git(dir: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let mut p = Proc::new("git");
    if let Some(d) = dir {
        p.arg("-C").arg(d);
    }
    p.args(args);
    let out = p
        .output()
        .map_err(|e| format!("git を実行できません: {e}\n  haj tree には git が必要です"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn die(msg: &str) -> ! {
    eprintln!("haj: {msg}");
    std::process::exit(1);
}

/// 補完(builtin::complete から呼ばれる)。
pub fn complete(words: &[String]) -> Vec<String> {
    match words.len() {
        0 => ["install", "update", "list", "remove"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        1 if matches!(words[0].as_str(), "update" | "remove") => {
            installed().into_iter().map(|(n, _)| n).collect()
        }
        _ => Vec::new(),
    }
}
