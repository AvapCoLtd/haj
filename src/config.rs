//! ユーザー設定(`~/.config/haj/config`)。
//!
//! 場所は XDG に従う。キャッシュが `~/.cache/haj/` にある以上、設定だけ `~/.haj/` に
//! 置くのは不整合。**git と同じ形**になる — リポジトリ側は `.haj/`(git の `.git/`)、
//! ユーザー側は `~/.config/haj/`(git の `~/.config/git/config`)。
//!
//! 形式は `.haj/project` と**同じ** `key = value`。設定ファイルの形式が2つあると、
//! 「どっちがどっちだったか」を覚える羽目になる。入れ子が要るような項目は今のところ
//! 無く、信頼済みツリーの一覧のような列は direnv 方式で別ファイルにすればよい。
//! これで依存クレートをゼロに保てる(YAML/TOML はパーサが要る)。

use std::collections::HashMap;
use std::path::PathBuf;

/// 設定値がどこから来たか。`haj config` で出す。
///
/// 環境変数 > 設定ファイル > 既定値、という3段の優先順位が**見えない**のは、
/// 「どの setup が走ったか分からない」のと同じ種類の欠陥。出所を必ず言う。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source {
    Env,
    File,
    Default,
}

impl Source {
    pub fn label(&self) -> &'static str {
        match self {
            Source::Env => "環境変数",
            Source::File => "設定ファイル",
            Source::Default => "既定値",
        }
    }
}

pub struct Config {
    map: HashMap<String, String>,
    pub path: Option<PathBuf>,
    pub exists: bool,
}

impl Config {
    pub fn load() -> Self {
        let path = config_dir().map(|d| d.join("config"));
        let (map, exists) = match &path {
            Some(p) => match std::fs::read_to_string(p) {
                Ok(s) => (parse_kv(&s), true),
                Err(_) => (HashMap::new(), false),
            },
            None => (HashMap::new(), false),
        };
        Self { map, path, exists }
    }

    /// 環境変数 > 設定ファイル > 既定値。
    pub fn get(&self, env_key: &str, file_key: &str, default: &str) -> (String, Source) {
        if let Ok(v) = std::env::var(env_key) {
            if !v.is_empty() {
                return (v, Source::Env);
            }
        }
        if let Some(v) = self.map.get(file_key) {
            if !v.is_empty() {
                return (v.clone(), Source::File);
            }
        }
        (default.to_string(), Source::Default)
    }

    /// 既定値を持たない値(トークンなど)。無ければ None。
    pub fn get_opt(&self, env_key: &str, file_key: &str) -> Option<(String, Source)> {
        if let Ok(v) = std::env::var(env_key) {
            if !v.is_empty() {
                return Some((v, Source::Env));
            }
        }
        self.map
            .get(file_key)
            .filter(|v| !v.is_empty())
            .map(|v| (v.clone(), Source::File))
    }
}

/// `$XDG_CONFIG_HOME/haj`(既定 `~/.config/haj`)。
pub fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|base| base.join("haj"))
}

/// コアが読む設定の一覧。`haj config` はこれを回して、**実効値と出所**を出す。
///
/// 環境変数 > 設定ファイル > 既定値、の3段が見えないと「なぜ効かないのか」を
/// 調べる手段が無くなる。`haj which` が探索順を見せるのと同じ理由でこれがある。
pub const KEYS: &[(&str, &str, &str)] = &[
    // (環境変数, 設定ファイルの鍵, 既定値)
    (
        "HAJ_COMMAND_PATH",
        "command_path",
        "/usr/local/lib/haj/commands",
    ),
    ("HAJ_HOOK_TIMEOUT_MS", "hook_timeout_ms", "2000"),
    ("HAJ_GITLAB", "gitlab", "https://gitlab.avaper.day"),
    ("HAJ_PROJECT_ID", "project_id", "788"),
    ("HAJ_TARGET", "target", "x86_64-unknown-linux-musl"),
];

/// `haj config` の出力。
///
/// `token` は KEYS に入れず、下で個別に扱う。既定値が無いのと、
/// **値そのものを出してはいけない**(シェルの履歴やスクショに残る)ため。
pub fn show() {
    let cfg = Config::load();

    match (&cfg.path, cfg.exists) {
        (Some(p), true) => println!("設定ファイル: {}", p.display()),
        (Some(p), false) => println!("設定ファイル: {} (まだありません)", p.display()),
        (None, _) => println!("設定ファイル: (HOME が分からないため特定できません)"),
    }
    println!();

    let width = KEYS
        .iter()
        .map(|(_, k, _)| k.len())
        .chain(std::iter::once("token".len()))
        .max()
        .unwrap_or(0);

    for (env_key, file_key, default) in KEYS {
        let (v, src) = cfg.get(env_key, file_key, default);
        println!(
            "  {file_key:width$}  {v}   ({})",
            src.label(),
            width = width
        );
    }

    // トークンは値を出さない。設定されているかどうかと、どこから来たかだけ言う。
    match cfg.get_opt("HAJ_TOKEN", "token") {
        Some((_, src)) => println!(
            "  {:width$}  ********   ({})",
            "token",
            src.label(),
            width = width
        ),
        None => println!("  {:width$}  (未設定)", "token", width = width),
    }

    println!();
    println!("環境変数 > 設定ファイル > 既定値 の順で決まります。");
    println!("形式は key = value ('#' から行末はコメント)。.haj/project と同じです。");
}

/// `key = value` を並べただけの形式。`#` から行末はコメント。
///
/// `.haj/project` と共用する。値は前後の空白と引用符を落とすだけで、
/// エスケープも型も無い。これ以上のものが要るなら、それは設定ファイルではなく
/// コマンドとして書くべきものだと考える。
pub fn parse_kv(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in content.lines() {
        let line = match line.find('#') {
            Some(i) => &line[..i],
            None => line,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let k = k.trim();
        let v = v.trim().trim_matches('"');
        if !k.is_empty() {
            map.insert(k.to_string(), v.to_string());
        }
    }
    map
}
