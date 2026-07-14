//! サブコマンドの探索。
//!
//! hajはディスパッチ表を持たない。「そこに置いてある実行可能ファイル」を探して
//! 見つける。プロジェクトごとに異なるサブコマンドのサブセットは、有効化リストの
//! ような別管理ではなく、この探索結果としてそのまま成立する。

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// $HAJ_COMMAND_PATH が未設定のときに探すシステム共通のコマンド置き場。
pub const DEFAULT_COMMAND_PATH: &str = "/usr/local/lib/haj/commands";

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
}

/// 探索対象の commands ディレクトリを、優先度の高い順に返す。
///
/// 1. カレントから上へ辿った各階層の `.haj/commands`   — プロジェクト固有
/// 2. `~/.haj/commands`                               — 個人用
/// 3. `$HAJ_COMMAND_PATH` の各ディレクトリ(コロン区切り) — イメージ/全社共通
pub fn command_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    // 1. カレントから上へ。ルートまで辿る(gitリポジトリの入れ子や、リポジトリの
    //    外で作業している場合でも素直に動くよう、境界は設けない)。
    if let Ok(cwd) = env::current_dir() {
        for ancestor in cwd.ancestors() {
            let d = ancestor.join(".haj").join("commands");
            if d.is_dir() {
                dirs.push(d);
            }
        }
    }

    // 2. 個人用
    if let Some(home) = home_dir() {
        let d = home.join(".haj").join("commands");
        if d.is_dir() {
            dirs.push(d);
        }
    }

    // 3. システム共通
    let system = env::var("HAJ_COMMAND_PATH").unwrap_or_else(|_| DEFAULT_COMMAND_PATH.to_string());
    for part in system.split(':').filter(|s| !s.is_empty()) {
        let d = PathBuf::from(part);
        if d.is_dir() {
            dirs.push(d);
        }
    }

    dirs
}

/// 名前からサブコマンドを解決する。探索順で最初に見つかったものが勝つ。
pub fn resolve(name: &str) -> Option<Command> {
    if !is_valid_name(name) {
        return None;
    }

    for dir in command_dirs() {
        let path = dir.join(name);
        if is_executable(&path) {
            return Some(Command {
                name: name.to_string(),
                root: root_of(&dir),
                path,
            });
        }
    }

    // 4. PATH上の haj-<name>(gitと同じ方式。逃げ道として残す)
    find_in_path(&format!("haj-{name}")).map(|path| Command {
        name: name.to_string(),
        path,
        root: None,
    })
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
        let root = root_of(&dir);
        let Ok(entries) = fs::read_dir(&dir) else {
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
            let path = dir.join(&name);
            push(
                Command {
                    name,
                    path,
                    root: root.clone(),
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
                is_valid_name(&name).then(|| (name, e.path()))
            })
            .collect();
        hits.sort();
        for (name, path) in hits {
            push(
                Command {
                    name,
                    path,
                    root: None,
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
    matches!(name, "help" | "commands" | "which" | "__complete")
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

fn find_in_path(exe: &str) -> Option<PathBuf> {
    path_dirs()
        .into_iter()
        .map(|d| d.join(exe))
        .find(|p| is_executable(p))
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}
