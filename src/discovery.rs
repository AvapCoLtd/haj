//! サブコマンドの探索。
//!
//! hajはディスパッチ表を持たない。「そこに置いてある実行可能ファイル」を探して
//! 見つける。プロジェクトごとに異なるサブコマンドのサブセットは、有効化リストの
//! ような別管理ではなく、この探索結果としてそのまま成立する。
//!
//! ただし素朴に `/` まで遡って全部積んではいけない。上流の野良 `.haj` が黙って
//! 効いてしまうし、どのプロジェクトの `setup` が走ったのか分からなくなる。
//! `.haj` を**境界**として扱う(project.rs 参照)。

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::project::{Origin, Project};

/// $HAJ_COMMAND_PATH が未設定のときに探すシステム共通のコマンド置き場。
pub const DEFAULT_COMMAND_PATH: &str = "/usr/local/lib/haj/commands";

/// 探索対象の commands ディレクトリと、その出自。
#[derive(Debug, Clone)]
pub struct Dir {
    pub path: PathBuf,
    pub origin: Origin,
}

/// 見つかったサブコマンド1つ。
#[derive(Debug, Clone)]
pub struct Command {
    /// 呼び出しに使う名前(`haj mig` の `mig`)
    pub name: String,
    /// 実行ファイルの絶対パス
    pub path: PathBuf,
    /// そのコマンドが属するツリー(commands/ の親)。
    /// サブコマンドには HAJ_ROOT として渡す。共通ライブラリを
    /// `$HAJ_ROOT/lib/...` から読めるようにするためのもの。
    /// PATH上の `haj-<name>` で見つかった場合は所属するツリーが無いので None。
    pub root: Option<PathBuf>,
    /// どこから来たコマンドか。一覧に出して素性を分かるようにする。
    pub origin: Origin,
}

/// いま自分がいるプロジェクト(カレントから遡って最初に見つかる `.haj`)。
///
/// 「どのプロジェクトに対して操作しているのか」を人にもサブコマンドにも
/// 明示するために使う。
pub fn active_project() -> Option<Project> {
    let cwd = env::current_dir().ok()?;
    let home = home_dir();
    for ancestor in cwd.ancestors() {
        // HOME はプロジェクトにしない。~/.haj が置いてあると遡上の途中で踏むが、
        // これをプロジェクト扱いすると、どのリポジトリにいても HOME が
        // プロジェクトとして効いてしまう。個人スコープは ~/.config/haj/ が正。
        if Some(ancestor) == home.as_deref() {
            continue;
        }
        if let Some(p) = Project::load(ancestor) {
            return Some(p);
        }
    }
    None
}

/// 探索対象の commands ディレクトリを、優先度の高い順に返す。
///
/// 1. カレントから上へ辿った `.haj/commands`   — **境界で止まる**
/// 2. `~/.config/haj/commands`                — 個人用
/// 3. `$HAJ_COMMAND_PATH` の各ディレクトリ     — 全社/イメージ共通
pub fn command_dirs() -> Vec<Dir> {
    let mut dirs = Vec::new();

    // 1. カレントから上へ(境界の規則は project_trees を参照)。
    for (tree, origin) in project_trees() {
        let d = tree.join("commands");
        if d.is_dir() {
            dirs.push(Dir { path: d, origin });
        }
    }

    // 2. 個人用。XDG に従い ~/.config/haj/commands。
    for d in user_command_dirs() {
        dirs.push(Dir {
            path: d,
            origin: Origin::User,
        });
    }

    // 3. インストール済みツリー(haj tree install。SPEC §9.5)
    for (name, dir) in crate::tree::installed() {
        let d = crate::tree::tree_root(&dir).join("commands");
        if d.is_dir() {
            dirs.push(Dir {
                path: d,
                origin: Origin::Tree(name),
            });
        }
    }

    // 4. システム共通
    let cfg = crate::config::Config::load();
    let (system, _) = cfg.get("HAJ_COMMAND_PATH", "command_path", DEFAULT_COMMAND_PATH);
    for part in system.split(':').filter(|s| !s.is_empty()) {
        let d = PathBuf::from(part);
        if d.is_dir() {
            dirs.push(Dir {
                path: d,
                origin: Origin::System,
            });
        }
    }

    dirs
}

/// カレントから上へ辿って見つかるプロジェクトのツリー(`.haj` ディレクトリ)を、
/// 優先度の高い順に返す。`commands/` の有無には依存しない — docs だけ置くツリーも
/// 正当(§9.3)。
///
/// `.haj` を持つディレクトリは既定でプロジェクト境界であり、そこで**止まる**。
/// `root = false` と書いたツリーだけが上へ抜ける(モノレポのサブプロジェクトが
/// 親の共通コマンドも継承したい場合)。止めないと、誰かが
/// `~/repos/.haj/commands/setup` を置いただけで、その配下の全リポジトリに
/// `haj setup` が生えてしまう。置いた本人以外は気づけない。
pub fn project_trees() -> Vec<(PathBuf, Origin)> {
    let mut trees = Vec::new();
    let home = home_dir();
    if let Ok(cwd) = env::current_dir() {
        for ancestor in cwd.ancestors() {
            if Some(ancestor) == home.as_deref() {
                continue; // HOME はプロジェクトにしない。個人スコープは ~/.config/haj/
            }
            let Some(proj) = Project::load(ancestor) else {
                continue;
            };
            trees.push((ancestor.join(".haj"), Origin::Project(proj.name.clone())));
            if proj.root {
                break; // ここが境界
            }
        }
    }
    trees
}

/// docs/ を持ちうるツリーを優先度の高い順に返す(§9.3)。
/// 探索順・境界はコマンドと同一だが、`commands/` の有無には依存しない。
/// システム共通は `$HAJ_COMMAND_PATH` の各エントリの親(`root_of` と同じ規則)。
pub fn doc_trees() -> Vec<(PathBuf, Origin)> {
    let mut trees = project_trees();

    if let Some(d) = crate::config::config_dir() {
        if d.is_dir() {
            trees.push((d, Origin::User));
        }
    }

    for (name, dir) in crate::tree::installed() {
        trees.push((crate::tree::tree_root(&dir), Origin::Tree(name)));
    }

    let cfg = crate::config::Config::load();
    let (system, _) = cfg.get("HAJ_COMMAND_PATH", "command_path", DEFAULT_COMMAND_PATH);
    for part in system.split(':').filter(|s| !s.is_empty()) {
        if let Some(root) = root_of(Path::new(part)) {
            trees.push((root, Origin::System));
        }
    }

    trees
}

/// 個人用コマンドの置き場所: `$XDG_CONFIG_HOME/haj/commands`(既定
/// `~/.config/haj/commands`)。ユーザー・スコープのツリーは ~/.config/haj/ に
/// 一本化されている(config / commands / docs — プロジェクトの .haj/ と対称)。
fn user_command_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(d) = crate::config::config_dir().map(|c| c.join("commands")) {
        if d.is_dir() {
            dirs.push(d);
        }
    }
    dirs
}

/// 名前からサブコマンドを解決する。探索順で最初に見つかったものが勝つ。
pub fn resolve(name: &str) -> Option<Command> {
    candidates(name).into_iter().next()
}

/// 同名のコマンドを、探索順に**すべて**返す。先頭が勝っているもの。
///
/// `haj which --all <名前>` の実体。同名が複数あるとき「どれが勝っていて、何が
/// 隠れているのか」が分からないままだと、破壊的なコマンドで事故る。
pub fn candidates(name: &str) -> Vec<Command> {
    if !is_valid_name(name) {
        return Vec::new();
    }

    let mut found = Vec::new();

    for dir in command_dirs() {
        let path = dir.path.join(name);
        if is_executable(&path) {
            found.push(Command {
                name: name.to_string(),
                root: root_of(&dir.path),
                path,
                origin: dir.origin,
            });
        }
    }

    // 4. PATH上の haj-<name>(gitと同じ方式。逃げ道として残す)
    if let Some(path) = find_in_path(&format!("haj-{name}")) {
        found.push(Command {
            name: name.to_string(),
            path,
            root: None,
            origin: Origin::Path,
        });
    }

    found
}

/// 使えるサブコマンドを全部列挙する。同名は探索順で先勝ち。名前順にソートして返す。
pub fn list() -> Vec<Command> {
    let mut found: Vec<Command> = Vec::new();
    let mut seen: Vec<String> = Vec::new();

    let push = |c: Command, seen: &mut Vec<String>, found: &mut Vec<Command>| {
        if !seen.contains(&c.name) {
            seen.push(c.name.clone());
            found.push(c);
        }
    };

    for dir in command_dirs() {
        let root = root_of(&dir.path);
        let Ok(entries) = fs::read_dir(&dir.path) else {
            continue;
        };
        let mut names: Vec<_> = entries
            .flatten()
            .filter(|e| is_executable(&e.path()))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| is_valid_name(n))
            .collect();
        names.sort();
        for name in names {
            let path = dir.path.join(&name);
            push(
                Command {
                    name,
                    path,
                    root: root.clone(),
                    origin: dir.origin.clone(),
                },
                &mut seen,
                &mut found,
            );
        }
    }

    // PATH上の haj-*
    for dir in path_dirs() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut hits: Vec<_> = entries
            .flatten()
            .filter(|e| is_executable(&e.path()))
            .filter_map(|e| {
                let f = e.file_name().into_string().ok()?;
                let name = f.strip_prefix("haj-")?.to_string();
                is_valid_name(&name).then_some((name, e.path()))
            })
            .collect();
        hits.sort();
        for (name, path) in hits {
            push(
                Command {
                    name,
                    path,
                    root: None,
                    origin: Origin::Path,
                },
                &mut seen,
                &mut found,
            );
        }
    }

    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// commands/ の親を HAJ_ROOT とする。ディレクトリ名が commands でなければ None。
fn root_of(command_dir: &Path) -> Option<PathBuf> {
    if command_dir.file_name() == Some(OsStr::new("commands")) {
        command_dir.parent().map(Path::to_path_buf)
    } else {
        None
    }
}

/// コマンド名として認めるもの。パス区切りや隠しファイル、コアが予約している名前を弾く。
///
/// 予約語を弾かないと、`.haj/commands/help` を置かれたときにコアのヘルプが
/// 奪われて「コマンド一覧が出せない」状態に陥りうる。
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && !name.starts_with('-')
        && !name.contains('/')
        && !is_reserved(name)
}

/// コアが自分で処理する名前。サブコマンドとしては使えない。
pub fn is_reserved(name: &str) -> bool {
    matches!(
        name,
        "help"
            | "commands"
            | "which"
            | "completion"
            | "config"
            | "docs"
            | "exec"
            | "sh"
            | "selfupgrade"
            | "secrets"
            | "tree"
            | "__complete"
    )
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        // シンボリックリンク切れなど
        return false;
    };
    meta.is_file() && meta.permissions().mode() & 0o111 != 0
}

fn path_dirs() -> Vec<PathBuf> {
    env::var_os("PATH")
        .map(|p| env::split_paths(&p).collect())
        .unwrap_or_default()
}

/// PATH からそのままの名前で探す。`haj exec`(§9.2)が使う。
pub fn find_in_path(exe: &str) -> Option<PathBuf> {
    path_dirs()
        .into_iter()
        .map(|d| d.join(exe))
        .find(|p| is_executable(p))
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}
