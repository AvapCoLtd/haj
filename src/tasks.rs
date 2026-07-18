//! プロジェクト・タスク(SPEC §9.6)。
//!
//! タスクはコマンドではない。コマンド(discovery)が「haj の語彙を増やすもの」で
//! 探索と上書きに乗るのに対し、タスクは**そのリポジトリの中でだけ意味を持つ作業**
//! (install / build / test のような作業動詞)。`haj run <名前>` が見るのは現在の
//! プロジェクトの `task.<名前>`(.haj/config)と `.haj/tasks/<名前>` だけ —
//! **探索しない・上書きしない・フォールバックしない**。この制約が「`haj run x` は
//! どの環境で読んでもそのリポジトリのタスク以外に解釈できない」という保証そのもの。
//!
//! `task.<名前>` の意味論はエイリアス(SPEC §2.7)と完全に同じ(「`haj` の後に
//! 打った語の並び」へ1回だけ展開・再帰なし)。config のタスクは「run 名前空間に
//! 住むエイリアス」であり、1行で書けなくなったら `tasks/` の実行ファイルに昇格する。

use std::collections::HashMap;
use std::path::PathBuf;

use crate::discovery::Command;
use crate::project::Origin;

/// タスク1つ。宣言(`task.*`)か実行ファイル(`tasks/`)のどちらか。
pub enum Task {
    /// `.haj/config` の `task.<名前> = <語...>`(1行の委譲)
    Decl {
        name: String,
        expansion: String,
        desc: Option<String>,
    },
    /// `.haj/tasks/<名前>`(実行ファイル。契約はコマンドと同じ)
    File(Command),
}

impl Task {
    pub fn name(&self) -> &str {
        match self {
            Task::Decl { name, .. } => name,
            Task::File(cmd) => &cmd.name,
        }
    }
}

/// 現在のプロジェクトの `.haj` と表示名。タスクはここ**だけ**を見る
/// (遡らない — `root = false` でも親のタスクは継承しない)。
pub fn project_haj() -> Option<(PathBuf, String)> {
    let p = crate::discovery::active_project()?;
    Some((p.dir.join(".haj"), p.name))
}

/// `.haj/config` を読む。`task.*` はホワイトリスト(SPEC §2.2)の一員で、
/// **プロジェクト config からしか読まない** — ユーザー設定・ツリー config の
/// `task.*` は無視される(タスクはプロジェクト局所の概念。どこでも効かせたい
/// ものは語彙 = エイリアスまたはコマンドとして定義する)。
fn decl_map() -> HashMap<String, String> {
    let Some((haj, _)) = project_haj() else {
        return HashMap::new();
    };
    match std::fs::read_to_string(haj.join("config")) {
        Ok(s) => crate::config::parse_kv(&s),
        Err(_) => HashMap::new(),
    }
}

/// 宣言(`task.<名前>`)を引く。名前の字面の制約は §2.6 と同じだが、
/// 予約語は弾かない(run 名前空間に組み込みは居ない — discovery::is_valid_ns_name)。
pub fn lookup_decl(name: &str) -> Option<Task> {
    if !crate::discovery::is_valid_ns_name(name) {
        return None;
    }
    let map = decl_map();
    let expansion = map
        .get(&format!("task.{name}"))
        .filter(|v| !v.is_empty())?
        .clone();
    Some(Task::Decl {
        name: name.to_string(),
        desc: map
            .get(&format!("task.{name}.desc"))
            .filter(|v| !v.is_empty())
            .cloned(),
        expansion,
    })
}

/// 実行ファイル(`.haj/tasks/<名前>`)を引く。実行可能の判定はコマンドと同じ(§2.5)。
pub fn lookup_file(name: &str) -> Option<Command> {
    if !crate::discovery::is_valid_ns_name(name) {
        return None;
    }
    let (haj, project) = project_haj()?;
    let path = haj.join("tasks").join(name);
    crate::discovery::is_executable(&path).then(|| Command {
        name: name.to_string(),
        path,
        // HAJ_ROOT はそのプロジェクトの .haj(SPEC §9.6)。共通 lib は
        // $HAJ_ROOT/lib/... から読める — コマンドと同じ型で書ける。
        root: Some(haj),
        origin: Origin::Project(project),
    })
}

/// 名前からタスクを引く。同名が両方にあれば**宣言が勝つ**
/// (予約語 > エイリアス > 探索、と同じ「宣言が手前」の並び)。
pub fn lookup(name: &str) -> Option<Task> {
    lookup_decl(name).or_else(|| lookup_file(name).map(Task::File))
}

/// タスクを全部返す(名前順)。宣言とファイルの同名は宣言が勝つ。
pub fn list() -> Vec<Task> {
    let Some((haj, project)) = project_haj() else {
        return Vec::new();
    };

    // 宣言(task.*)。規則は aliases_in と同じ(.desc は説明であってタスクではない)
    let map = decl_map();
    let mut out: Vec<Task> = map
        .iter()
        .filter_map(|(k, v)| {
            let name = k.strip_prefix("task.")?;
            (!name.ends_with(".desc") && !v.is_empty() && crate::discovery::is_valid_ns_name(name))
                .then(|| Task::Decl {
                    name: name.to_string(),
                    expansion: v.clone(),
                    desc: map
                        .get(&format!("task.{name}.desc"))
                        .filter(|d| !d.is_empty())
                        .cloned(),
                })
        })
        .collect();

    // 実行ファイル(.haj/tasks/)。宣言に同名があれば出さない(宣言が勝つ)
    let dir = haj.join("tasks");
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut names: Vec<String> = entries
            .flatten()
            .filter(|e| crate::discovery::is_executable(&e.path()))
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| crate::discovery::is_valid_ns_name(n))
            .collect();
        names.sort();
        for name in names {
            if out.iter().any(|t| t.name() == name) {
                continue;
            }
            out.push(Task::File(Command {
                path: dir.join(&name),
                name,
                root: Some(haj.clone()),
                origin: Origin::Project(project.clone()),
            }));
        }
    }

    out.sort_by(|a, b| a.name().cmp(b.name()));
    out
}
