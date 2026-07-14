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

    /// エイリアス(SPEC §2.7)。`alias.<名前> = <語...>`。
    /// **ユーザー設定だけ**から読む — リポジトリ側(.haj/project 等)に定義させると、
    /// clone したリポジトリが `alias.mig = sh '...'` を仕込めてしまう。
    pub fn alias(&self, name: &str) -> Option<String> {
        self.map
            .get(&format!("alias.{name}"))
            .filter(|v| !v.is_empty())
            .cloned()
    }

    /// 定義済みエイリアスの一覧(名前順)。予約語の名前は無視される側なので除く。
    pub fn aliases(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self
            .map
            .iter()
            .filter_map(|(k, val)| {
                let name = k.strip_prefix("alias.")?;
                (!name.is_empty() && !val.is_empty() && !crate::discovery::is_reserved(name))
                    .then(|| (name.to_string(), val.clone()))
            })
            .collect();
        v.sort();
        v
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

/// コアが読む設定の一覧。`haj config` はこれを回して**実効値と出所**を出し、
/// `haj config --init` は**雛形**を出す。
///
/// 環境変数 > 設定ファイル > 既定値、の3段が見えないと「なぜ効かないのか」を
/// 調べる手段が無くなる。`haj which` が探索順を見せるのと同じ理由でこれがある。
pub const KEYS: &[(&str, &str, &str, &str)] = &[
    // (環境変数, 設定ファイルの鍵, 既定値, 説明)
    (
        "HAJ_COMMAND_PATH",
        "command_path",
        "/usr/local/lib/haj/commands",
        "システム共通のコマンド置き場 (':' 区切り)",
    ),
    (
        "HAJ_HOOK_TIMEOUT_MS",
        "hook_timeout_ms",
        "2000",
        "規約フック (--haj-describe 等) のタイムアウト (ミリ秒)",
    ),
    (
        "HAJ_OP_CMD",
        "secrets.op_cmd",
        "op",
        "op 参照の解決に使う CLI",
    ),
    (
        "HAJ_VAULT_CMD",
        "secrets.vault_cmd",
        crate::secrets::DEFAULT_VAULT_CMD,
        "vault 参照の解決に使う CLI",
    ),
    (
        "VAULT_ADDR",
        "secrets.vault_addr",
        crate::secrets::DEFAULT_VAULT_ADDR,
        "vault サーバ (環境の VAULT_ADDR / BAO_ADDR が優先)",
    ),
    (
        "HAJ_VAULT_LOGIN",
        "secrets.vault_login",
        crate::secrets::DEFAULT_VAULT_LOGIN,
        "未ログイン時に自動実行する login の引数。off で無効化",
    ),
    (
        "HAJ_GITHUB",
        "selfupgrade.github",
        crate::selfupgrade::DEFAULT_GITHUB,
        "haj 自身の取得元 GitHub リポジトリ (public。認証不要)",
    ),
    (
        "HAJ_GITLAB",
        "selfupgrade.gitlab",
        crate::selfupgrade::DEFAULT_GITLAB,
        "private な取得元を使うとき: GitLab インスタンス",
    ),
    (
        "HAJ_PROJECT_ID",
        "selfupgrade.project_id",
        crate::selfupgrade::DEFAULT_PROJECT_ID,
        "private な取得元を使うとき: GitLab のプロジェクト ID",
    ),
    (
        "HAJ_TARGET",
        "selfupgrade.target",
        "x86_64-unknown-linux-musl",
        "取得するビルドのターゲット",
    ),
];

/// 鍵の名前空間(ドットの前)。git の `user.name` と同じ流儀で、TOML を
/// 持ち込まずにグループ分けする。ドット無しはコア。
fn group_of(file_key: &str) -> &str {
    match file_key.split_once('.') {
        Some((g, _)) => g,
        None => "",
    }
}

fn group_title(group: &str) -> &str {
    match group {
        "" => "コア (探索と規約)",
        "secrets" => "secrets: シークレット参照の解決 (SPEC §10)",
        "selfupgrade" => "selfupgrade: haj自身の更新 (SPEC §9.1)",
        other => other,
    }
}

/// `haj config --init` — 設定できる鍵と既定値をすべて、設定ファイルの雛形として出す。
///
/// 全行コメントなので、そのまま置いても挙動は変わらない。変えたい行だけ
/// コメントを外す:  haj config --init > ~/.config/haj/config
pub fn template() {
    println!("# haj の設定");
    println!("# 形式: key = value。'#' から行末はコメント。行末の '\\' は継続。すべて省略可。");
    println!("# 優先順位: 環境変数 > 設定ファイル > 既定値。実効値は `haj config` で確認。");
    let mut group = None;
    for (env_key, file_key, default, desc) in KEYS {
        let g = group_of(file_key);
        if group != Some(g) {
            group = Some(g);
            println!();
            println!("# ------ {} ------", group_title(g));
        }
        println!();
        println!("# {desc} (環境変数: {env_key})");
        println!("# {file_key} = {default}");
    }
    println!();
    println!("# ------ alias: エイリアス (git 方式。SPEC §2.7) ------");
    println!();
    println!("# alias.<名前> = <語...>  名前が語の並びに展開され、残りの引数が続く");
    println!("# alias.ie = -C ~/repos/example-app");
    println!("#");
    println!("# 長いものは行末の '\\' で継続できる:");
    println!("# alias.oci = --secret OCI_CLI_USER=vault://users/me/oci/user \\");
    println!("#             --secret-file OCI_CLI_KEY_FILE=vault://users/me/oci/private_key \\");
    println!("#             exec oci");
    println!();
    println!("# private な取得元(GitLab)を使うときのトークン (環境変数: HAJ_TOKEN)。");
    println!("# 平文でも、シークレット参照でもよい (SPEC §8.4):");
    println!("# selfupgrade.token = <トークン>");
    println!("# selfupgrade.token = vault://<マウント>/<パス>/token");
}

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
        .map(|(_, k, _, _)| k.len())
        .chain(std::iter::once("selfupgrade.token".len()))
        .max()
        .unwrap_or(0);

    let mut group = None;
    for (env_key, file_key, default, _) in KEYS {
        let g = group_of(file_key);
        if group.is_some() && group != Some(g) {
            println!(); // グループの切れ目
        }
        group = Some(g);
        let (v, src) = cfg.get(env_key, file_key, default);
        println!(
            "  {file_key:width$}  {v}   ({})",
            src.label(),
            width = width
        );
    }

    // トークンの実体は出さない。ただし**参照**なら参照をそのまま出す —
    // 参照は秘密ではないし、どこの金庫を指しているかは調べたい情報(SPEC §8.4)。
    match cfg.get_opt("HAJ_TOKEN", "selfupgrade.token") {
        Some((v, src)) if crate::secrets::is_reference(&v) => println!(
            "  {:width$}  {v}   ({})",
            "selfupgrade.token",
            src.label(),
            width = width
        ),
        Some((_, src)) => println!(
            "  {:width$}  ********   ({})",
            "selfupgrade.token",
            src.label(),
            width = width
        ),
        None => println!("  {:width$}  (未設定)", "selfupgrade.token", width = width),
    }

    println!();
    println!("環境変数 > 設定ファイル > 既定値 の順で決まります。");
    println!("形式は key = value ('#' から行末はコメント)。.haj/project と同じです。");
}

/// `key = value` を並べただけの形式。`#` から行末はコメント。
/// **行末の `\` は継続**(シェルや git config と同じ)。継続行は空白1つで繋がる。
///
/// `.haj/project` と共用する。値は前後の空白と引用符を落とすだけで、
/// エスケープも型も無い。これ以上のものが要るなら、それは設定ファイルではなく
/// コマンドとして書くべきものだと考える。
pub fn parse_kv(content: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let mut pending = String::new();

    for raw in content.lines() {
        // コメントを落としてから継続を見る(`#` の後ろの `\` は継続ではない)
        let line = match raw.find('#') {
            Some(i) => &raw[..i],
            None => raw,
        };
        let line = line.trim();
        if line.is_empty() && pending.is_empty() {
            continue;
        }

        // 行末の `\` は継続(シェルや git config と同じ)。長い alias が1行に
        // 収まらないと読めないし書けない。継続行は空白1つで繋ぐ。
        if let Some(head) = line.strip_suffix('\\') {
            if !pending.is_empty() {
                pending.push(' ');
            }
            pending.push_str(head.trim_end());
            continue;
        }

        let joined = if pending.is_empty() {
            line.to_string()
        } else {
            let mut j = std::mem::take(&mut pending);
            j.push(' ');
            j.push_str(line);
            j
        };

        let Some((k, v)) = joined.split_once('=') else {
            continue;
        };
        let k = k.trim();
        let v = v.trim().trim_matches('"');
        if !k.is_empty() {
            map.insert(k.to_string(), v.to_string());
        }
    }

    // 継続で終わったまま(最後の行が `\`)なら、そこまでを使う
    if !pending.is_empty() {
        if let Some((k, v)) = pending.split_once('=') {
            let k = k.trim();
            let v = v.trim().trim_matches('"');
            if !k.is_empty() {
                map.insert(k.to_string(), v.to_string());
            }
        }
    }
    map
}
