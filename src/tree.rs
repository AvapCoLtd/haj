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
使い方: haj tree install <gitのURL>[@<ref>] [--name <名前>] [--global]
        haj tree update [<名前>]
        haj tree list
        haj tree remove <名前>";

/// 個人のインストール先: `$XDG_DATA_HOME/haj/trees`(既定 `~/.local/share/haj/trees`)。
/// 設定でも環境変数でもなくデータなので、XDG data に置く。
pub fn trees_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
        .map(|base| base.join("haj").join("trees"))
}

/// システム共通のインストール先: `$XDG_DATA_DIRS` の各エントリ(既定
/// `/usr/local/share:/usr/share`)の `haj/trees`。`--global` は先頭に入れる。
/// イメージに焼くとき(Dockerfile の RUN haj tree install --global ...)のためにある。
pub fn global_trees_dirs() -> Vec<PathBuf> {
    let dirs = std::env::var("XDG_DATA_DIRS").unwrap_or_default();
    let dirs = if dirs.is_empty() {
        "/usr/local/share:/usr/share"
    } else {
        &dirs
    };
    dirs.split(':')
        .filter(|d| !d.is_empty())
        .map(|d| PathBuf::from(d).join("haj").join("trees"))
        .collect()
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

/// ツリーの config の `expose`(SPEC §9.7)。`namespace` なら素の探索から外れ、
/// 名前空間(`haj <ツリー名> <名前>`)でだけ呼べる。既定は flat(従来どおり)。
pub fn is_namespaced(dir: &Path) -> bool {
    let Ok(s) = std::fs::read_to_string(tree_root(dir).join("config")) else {
        return false;
    };
    crate::config::parse_kv(&s)
        .get("expose")
        .map(String::as_str)
        == Some("namespace")
}

/// 名前からインストール済みツリーを引く(名前空間ディスパッチ用。SPEC §9.7)。
pub fn find(name: &str) -> Option<PathBuf> {
    installed()
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, d)| d)
}

/// `expose = namespace` を宣言したツリーだけを返す(help の入口表示用)。
pub fn namespaced() -> Vec<(String, PathBuf)> {
    installed()
        .into_iter()
        .filter(|(_, d)| is_namespaced(d))
        .collect()
}

/// そのツリーのコマンドを名前順に返す(名前空間の一覧・補完用)。
pub fn tree_commands(tree: &str, dir: &Path) -> Vec<crate::discovery::Command> {
    let root = tree_root(dir);
    let cdir = root.join("commands");
    let Ok(entries) = std::fs::read_dir(&cdir) else {
        return Vec::new();
    };
    let mut v: Vec<crate::discovery::Command> = entries
        .flatten()
        .filter(|e| crate::discovery::is_executable(&e.path()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| crate::discovery::is_valid_ns_name(n))
        .map(|n| crate::discovery::Command {
            path: cdir.join(&n),
            name: n,
            root: Some(root.clone()),
            origin: crate::project::Origin::Tree(tree.to_string()),
        })
        .collect();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

/// そのツリーの1コマンドを引く。名前の字面の制約は §2.6 と同じだが、
/// 予約語は弾かない(名前空間の中に組み込みは居ない — §9.7)。
pub fn tree_command(tree: &str, dir: &Path, name: &str) -> Option<crate::discovery::Command> {
    if !crate::discovery::is_valid_ns_name(name) {
        return None;
    }
    let root = tree_root(dir);
    let path = root.join("commands").join(name);
    crate::discovery::is_executable(&path).then(|| crate::discovery::Command {
        name: name.to_string(),
        path,
        root: Some(root),
        origin: crate::project::Origin::Tree(tree.to_string()),
    })
}

/// インストール済みツリーを名前順に返す(名前 = ディレクトリ名)。
/// 個人 > グローバルの順で見て、同名は近いスコープが勝つ(コマンド探索と同じ規律)。
pub fn installed() -> Vec<(String, PathBuf)> {
    let mut bases: Vec<PathBuf> = Vec::new();
    bases.extend(trees_dir());
    bases.extend(global_trees_dirs());

    let mut v: Vec<(String, PathBuf)> = Vec::new();
    for base in bases {
        let Ok(entries) = std::fs::read_dir(&base) else {
            continue;
        };
        for e in entries.flatten() {
            if !e.path().is_dir() {
                continue;
            }
            let Ok(n) = e.file_name().into_string() else {
                continue;
            };
            if n.starts_with('.') || v.iter().any(|(name, _)| *name == n) {
                continue;
            }
            v.push((n, e.path()));
        }
    }
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
    let mut global = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--name" {
            let Some(n) = it.next() else {
                die(&format!("--name には値が要ります\n{USAGE}"));
            };
            name_flag = Some(n.clone());
        } else if a == "--global" {
            global = true;
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

    let base = if global {
        let Some(b) = global_trees_dirs().into_iter().next() else {
            die("グローバルの置き場が分かりません ($XDG_DATA_DIRS)");
        };
        b
    } else {
        let Some(b) = trees_dir() else {
            die("HOME が分かりません");
        };
        b
    };
    if let Err(e) = std::fs::create_dir_all(&base) {
        die(&format!(
            "{} を作れません: {e}{}",
            base.display(),
            if global {
                "\n  --global には書き込み権限が要ります (sudo など)"
            } else {
                ""
            }
        ));
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

    // この名前は名前空間の名にもなる(§9.7)。既存の語彙と衝突していたら教える
    // (入れるのは止めない — --name で改名すればよい)。
    if crate::discovery::is_reserved(&name) {
        eprintln!("warning: {name} は予約語なので haj {name} では呼べません (remove して --name で改名を)");
    } else if crate::aliases::lookup(&name).is_some() || crate::discovery::resolve(&name).is_some()
    {
        eprintln!(
            "warning: {name} は既存の語彙と衝突しています (haj which --all {name} で確認。名前空間が勝ちます)"
        );
    }
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
        let ns = if is_namespaced(&dir) {
            "  (namespace)"
        } else {
            ""
        };
        println!("{name:width$}  {head:8}  {n:3} コマンド  {url}{ns}");
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
