//! プロジェクトの境界と素性。
//!
//! 探索を素朴に `/` まで遡って全部積むと、二つの困りごとが起きる。
//!
//! 1. **上流の野良 `.haj` が黙って効く。** 誰かが `~/repos/.haj/commands/setup` を
//!    置くと、その配下の全リポジトリで `haj setup` が生えてしまう。置いた本人以外
//!    気づけない。
//! 2. **どのプロジェクトの `setup` が走ったのか分からない。** モノレポで同名の
//!    コマンドがあると、近い方が勝つが、出力からは区別がつかない。
//!    setup/reset は破壊的なので、これは事故になる。
//!
//! そこで `.haj` を**プロジェクト境界**として扱う。既定では最初に見つけた `.haj` で
//! 遡上を止める。親の共通コマンドも継承したい入れ子(モノレポのサブプロジェクト)
//! だけが `.haj/config` に `root = false` と書いて、明示的に上へ抜ける。

use std::fs;
use std::path::{Path, PathBuf};

/// `.haj/config` の内容。ファイルが無くても既定値で成立する。
#[derive(Debug, Clone)]
pub struct Project {
    /// 表示名。既定は `.haj` を含むディレクトリの名前。
    pub name: String,
    /// `.haj` を含むディレクトリ(= リポジトリのルート)
    pub dir: PathBuf,
    /// ここで遡上を止めるか。既定 true(=境界)。
    /// `root = false` にすると、さらに上の `.haj` も探しに行く。
    pub root: bool,
}

impl Project {
    /// `<dir>/.haj` から読む。`.haj` が無ければ None。
    pub fn load(dir: &Path) -> Option<Self> {
        let haj = dir.join(".haj");
        if !haj.is_dir() {
            return None;
        }

        let default_name = dir
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| dir.display().to_string());

        let mut p = Project {
            name: default_name,
            dir: dir.to_path_buf(),
            root: true,
        };

        // `.haj/config` は `key = value` を並べただけの素朴な形式。ユーザー設定
        // (~/.config/haj/config)と**同じ**パーサ・同じファイル名 — ツリー構成が
        // スコープ間で対称になる(<scope>/config + <scope>/commands/ + <scope>/docs/)。
        //
        // ただしプロジェクト・スコープで効く鍵は**ホワイトリスト**(name / root /
        // alias.*)。secrets.* や selfupgrade.* をここから読むと、clone した
        // リポジトリに金庫や更新元の接続先を乗っ取られる。接続先を変える鍵は
        // ユーザー設定と環境変数からしか読まない(~/.gitconfig と .git/config の関係)。
        if let Ok(content) = fs::read_to_string(haj.join("config")) {
            let kv = crate::config::parse_kv(&content);
            if let Some(name) = kv.get("name").filter(|v| !v.is_empty()) {
                p.name = name.clone();
            }
            if let Some(root) = kv.get("root") {
                p.root = root != "false";
            }
        }

        Some(p)
    }
}

/// コマンドの出自。一覧に出して「どこの誰か」を分かるようにする。
#[derive(Debug, Clone, PartialEq)]
pub enum Origin {
    /// あるプロジェクトのもの(`.haj/commands`)
    Project(String),
    /// 個人用(`~/.config/haj/commands`)
    User,
    /// 全社/イメージ共通(`$HAJ_COMMAND_PATH`)
    System,
    /// PATH 上の `haj-<名前>`
    Path,
    /// コア組み込み(`help` / `commands` / `which` / `selfupgrade`)。
    /// 探索されないが、どこにいても使えるので一覧には出す。
    Core,
}

impl Origin {
    /// 一覧の右端に出すラベル。
    pub fn label(&self) -> String {
        match self {
            Origin::Project(name) => format!("[{name}]"),
            Origin::User => "[個人]".to_string(),
            Origin::System => "[共通]".to_string(),
            Origin::Path => "[PATH]".to_string(),
            Origin::Core => "[haj]".to_string(),
        }
    }
}
