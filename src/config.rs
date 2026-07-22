//! ユーザー設定(`~/.config/haj/config`)。
//!
//! 場所は XDG に従う。キャッシュが `~/.cache/haj/` にある以上、設定だけ `~/.haj/` に
//! 置くのは不整合。**git と同じ形**になる — リポジトリ側は `.haj/`(git の `.git/`)、
//! ユーザー側は `~/.config/haj/`(git の `~/.config/git/config`)。
//!
//! 形式は `.haj/config` と**同じ** `key = value`。設定ファイルの形式が2つあると、
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

    /// エイリアス(SPEC §2.7)の**ユーザー設定スコープ**。
    /// プロジェクトの `.haj/config` も含めた解決は aliases::lookup が行う
    /// (近いスコープが勝つ)。
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
                // `alias.<名前>.desc` は説明であってエイリアスではない
                (!name.is_empty()
                    && !name.ends_with(".desc")
                    && !val.is_empty()
                    && !crate::discovery::is_reserved(name))
                .then(|| (name.to_string(), val.clone()))
            })
            .collect();
        v.sort();
        v
    }

    /// `alias.<名前>.desc` — 一覧と補完に出す一行説明。
    /// 長いエイリアスは展開をそのまま出すと読めないので、これを書ける。
    pub fn alias_desc(&self, name: &str) -> Option<String> {
        self.map
            .get(&format!("alias.{name}.desc"))
            .filter(|v| !v.is_empty())
            .cloned()
    }

    /// ツリーごとの設定注入(SPEC §10.8)。`tree.<名前>.<kind>.KEY = 値` を
    /// (KEY, 値) で名前順に返す。kind は "env"(平文・無展開)か "secret"(参照)。
    ///
    /// **ユーザー設定からだけ**読む。ツリー自身の config やプロジェクトの
    /// .haj/config に書かれた tree.* をここが見ることは無い — clone した木が
    /// 自分への注入を宣言できると、盗み先の指定になる(SPEC §10.8)。
    pub fn tree_entries(&self, tree: &str, kind: &str) -> Vec<(String, String)> {
        let prefix = format!("tree.{tree}.{kind}.");
        let mut v: Vec<(String, String)> = self
            .map
            .iter()
            .filter_map(|(k, val)| {
                let key = k.strip_prefix(&prefix)?;
                (!key.is_empty() && !key.contains('.') && !val.is_empty())
                    .then(|| (key.to_string(), val.clone()))
            })
            .collect();
        v.sort();
        v
    }

    /// 鍵が設定ファイルに書かれているか(値の中身は見ない)。
    /// 改名された旧キーの警告(store.rs)に使う。
    pub fn has_key(&self, file_key: &str) -> bool {
        self.map.contains_key(file_key)
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
        "HAJ_DOCS_FZF_CMD",
        "docs.fzf_cmd",
        "fzf",
        "docs の選択UIに使うコマンド (語分割。先頭がバイナリ)",
    ),
    (
        "HAJ_DOCS_FZF_ARGS",
        "docs.fzf_args",
        "",
        "選択UIへ追加で渡す引数 (haj の既定の後ろに付く。fzf は後勝ち)",
    ),
    (
        "HAJ_DOCS_PREVIEW_CMD",
        "docs.preview_cmd",
        "",
        "プレビューのフィルタ。markdown を stdin で受ける (例: glow -)",
    ),
    (
        "HAJ_DOCS_PAGER",
        "docs.pager",
        "",
        "Enter で開くビューア (既定: $PAGER、無ければ less)",
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
        "HAJ_STORE_TREE_ENGINE",
        "store.tree.engine",
        crate::store::DEFAULT_ENGINE,
        "ストア tree のエンジン (v1 は vault のみ。SPEC §10.7)",
    ),
    (
        "HAJ_STORE_TREE_PREFIX",
        "store.tree.prefix",
        crate::store::DEFAULT_PREFIX_DOC,
        "ストア tree の物理プレフィックス (書式は vault:// のパスと同じ。<ユーザー名> は実行ユーザーで埋まる)",
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
        "docs" => "docs: ドキュメントの選択UI (SPEC §9.3)",
        "secrets" => "secrets: シークレット参照の解決 (SPEC §10)",
        "store" => "store: ストアの表 (v1 は予約行 tree のみ。SPEC §10.7)",
        "selfupgrade" => "selfupgrade: haj自身の更新 (SPEC §9.1)",
        "meta" => "meta: ユーザー定義 (コアは解釈しない。SPEC §8.5)",
        other => other,
    }
}

/// `haj config get <キー>`(SPEC §8.5)— 実効値を1行で stdout へ。
///
/// コアが知る鍵(KEYS)は 環境変数 > 設定ファイル > 既定値。それ以外
/// (`meta.*` / `tree.*` / `alias.*` など)はユーザー設定ファイルの値。
/// 未設定・未知は exit 1(fail-fast — 呼び出し側の `${VAR:-...}` 定石に落ちる)。
pub fn get_value(key: &str) -> ! {
    let cfg = Config::load();
    // token の平文は出さない(§8.3 の不変条件を plumbing でも破らない)。
    // 参照なら出す — 参照は秘密ではない(§8.4)。
    if key == "selfupgrade.token" {
        match cfg.get_opt("HAJ_TOKEN", "selfupgrade.token") {
            Some((v, _)) if crate::secrets::is_reference(&v) => {
                println!("{v}");
                std::process::exit(0);
            }
            Some(_) => {
                eprintln!("haj: selfupgrade.token の平文は出せません (参照なら出ます — SPEC §8.4)");
                std::process::exit(1);
            }
            None => {
                eprintln!("haj: 未設定です: {key}");
                std::process::exit(1);
            }
        }
    }
    if let Some((env_key, file_key, default, _)) = KEYS.iter().find(|(_, k, _, _)| *k == key) {
        let (v, _) = cfg.get(env_key, file_key, default);
        if v.is_empty() {
            eprintln!("haj: 未設定です: {key}");
            std::process::exit(1);
        }
        println!("{v}");
        std::process::exit(0);
    }
    match cfg.map.get(key).filter(|v| !v.is_empty()) {
        Some(v) => {
            println!("{v}");
            std::process::exit(0);
        }
        None => {
            eprintln!("haj: 未設定です: {key}");
            std::process::exit(1);
        }
    }
}

/// `haj config set <キー> <値>`(SPEC §8.5)— ユーザー設定へ書く。
///
/// 保存先は常にユーザー設定(人がコマンドを打つこと自体が同意 — §10.8 と
/// 同じ層)。既存キーはその**論理行**(継続行ごと)を置換し、他の行と
/// コメントは保つ。無ければ末尾に追記する。
pub fn set_value(key: &str, value: &str) -> ! {
    let valid = !key.is_empty()
        && !key.starts_with('.')
        && !key.ends_with('.')
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if !valid {
        eprintln!("haj: キーにできません: {key} (使えるのは英数字と . _ -)");
        std::process::exit(1);
    }
    let Some(path) = config_dir().map(|d| d.join("config")) else {
        eprintln!("haj: HOME が分かりません");
        std::process::exit(1);
    };
    let old = std::fs::read_to_string(&path).unwrap_or_default();

    let mut out: Vec<String> = Vec::new();
    let mut replaced = false;
    let mut in_continuation = false;
    let mut drop_continuation = false;
    for raw in old.lines() {
        // 継続の途中の行: 直前の論理行の続き。置換対象なら一緒に落とす
        if in_continuation {
            in_continuation = line_continues(raw);
            if !drop_continuation {
                out.push(raw.to_string());
            }
            continue;
        }
        in_continuation = line_continues(raw);
        drop_continuation = false;
        let head = match raw.find('#') {
            Some(i) => &raw[..i],
            None => raw,
        };
        let is_target = head.split_once('=').is_some_and(|(k, _)| k.trim() == key);
        if is_target {
            // 最初の1回だけ置換し、同じキーの残りは落とす(重複を残さない)
            if !replaced {
                out.push(format!("{key} = {value}"));
                replaced = true;
            }
            drop_continuation = true;
            continue;
        }
        out.push(raw.to_string());
    }
    if !replaced {
        out.push(format!("{key} = {value}"));
    }

    if let Some(p) = path.parent() {
        let _ = std::fs::create_dir_all(p);
    }
    let body = out.join("\n") + "\n";
    if let Err(e) = std::fs::write(&path, body) {
        eprintln!("haj: {} に書けません: {e}", path.display());
        std::process::exit(1);
    }
    println!(
        "{}: {key} = {value} ({})",
        if replaced {
            "置き換えました"
        } else {
            "設定しました"
        },
        path.display()
    );
    std::process::exit(0);
}

/// 行がコメント外の `\` で終わる = 次の行へ継続する(parse_kv と同じ規則)。
fn line_continues(raw: &str) -> bool {
    let head = match raw.find('#') {
        Some(i) => &raw[..i],
        None => raw,
    };
    head.trim_end().ends_with('\\')
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
    println!("# ------ tree: ツリーごとの設定注入 (SPEC §10.8) ------");
    println!();
    println!("# tree.<インストール名>.env.KEY    = <値>    平文をそのまま注入 (一切展開しない)");
    println!(
        "# tree.<インストール名>.secret.KEY = <参照>  実行時に解決して注入 (参照以外はエラー)"
    );
    println!("# 優先順位: フラグ > シェル環境 > tree設定 > コマンドの既定値。");
    println!("# 実効値と出所は haj env <ツリー名> <コマンド> で確認できる。");
    println!("#");
    println!("# tree.work.env.MYAPP_ACCOUNT    = alice@example.com");
    println!("# tree.work.env.TOKEN_OUTPUT     = store://token   # 参照もただの文字列として渡る");
    println!("# tree.work.secret.CLIENT_SECRET = vault://secret/data/myapp/client_secret");
    println!();
    println!("# ------ meta: ユーザー定義 (コアは解釈しない。SPEC §8.5) ------");
    println!();
    println!("# ツリー間で共有する「本人についての値」の置き場。haj config get/set で読み書き。");
    println!("# meta.username = <金庫でのユーザー名>   # 例: ツリーの config-init が個人パスの生成に使う");
    println!();
    println!("# private な取得元(GitLab)を使うときのトークン (環境変数: HAJ_TOKEN)。");
    println!("# 平文でも、シークレット参照でもよい (SPEC §8.4):");
    println!("# selfupgrade.token = <トークン>");
    println!("# selfupgrade.token = vault://<マウント>/<パス>/token");
}

/// 補完用: コアが知る鍵 + ユーザー設定にある鍵(重複なし・名前順)。
pub fn known_keys() -> Vec<String> {
    let mut v: Vec<String> = KEYS.iter().map(|(_, k, _, _)| k.to_string()).collect();
    v.push("selfupgrade.token".to_string());
    let cfg = Config::load();
    for k in cfg.map.keys() {
        if !v.contains(k) {
            v.push(k.clone());
        }
    }
    v.sort();
    v
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

    // meta.* — ユーザー定義域(§8.5)。コアは解釈しないが、あることは見せる
    let mut metas: Vec<(&String, &String)> = cfg
        .map
        .iter()
        .filter(|(k, _)| k.starts_with("meta."))
        .collect();
    if !metas.is_empty() {
        metas.sort();
        println!();
        for (k, v) in metas {
            println!("  {k:width$}  {v}   (設定ファイル)", width = width);
        }
    }

    println!();
    println!("環境変数 > 設定ファイル > 既定値 の順で決まります。");
    println!("形式は key = value ('#' から行末はコメント)。.haj/config と同じです。");
}

/// `haj config --tree <インストール名>` — そのツリーインスタンスの**全景**
/// (SPEC §10.8)。人手の点検用で、金庫には触らない。
///
/// - `.env` は実効値と出所: シェル環境に同名があればそちらが勝つ(§10.8 の
///   優先順位のとおり。フラグはその場の明示なのでここには出ない)
/// - `.secret` は**参照のまま**(宣言 — 解決しない。値は `haj secret get`)
/// - store の名前空間(§10.7 の写像先)も添える — このインスタンスの
///   `haj store` / `store://` がどこへ行くのかの答え
/// - `effective_env` は各コマンドの `--haj-env` の節連結(呼び出し側が用意)。
///   コマンド自身の既定値まで含む実効 env — 「全景」はこれで閉じる
pub fn show_tree(tree: &str, effective_env: Option<&str>) {
    let cfg = Config::load();

    let installed = crate::tree::installed().iter().any(|(n, _)| n == tree);
    if installed {
        println!("ツリー: {tree}");
    } else {
        println!("ツリー: {tree}  (未インストール — 設定だけがある状態)");
    }
    println!(
        "store の名前空間: {}",
        crate::store::namespace_display(tree)
    );

    let envs = cfg.tree_entries(tree, "env");
    let secs = cfg.tree_entries(tree, "secret");
    if envs.is_empty() && secs.is_empty() {
        println!();
        println!("このツリーの設定はありません (tree.{tree}.*)。");
        println!("  tree.{tree}.env.KEY    = <値>    平文を環境変数として注入");
        println!("  tree.{tree}.secret.KEY = <参照>  宣言 (haj secret get <KEY> で引く)");
        show_effective_env(tree, installed, effective_env);
        return;
    }

    let width = envs
        .iter()
        .map(|(k, _)| format!("tree.{tree}.env.{k}").len())
        .chain(
            secs.iter()
                .map(|(k, _)| format!("tree.{tree}.secret.{k}").len()),
        )
        .max()
        .unwrap_or(0);

    if !envs.is_empty() {
        println!();
        for (k, v) in &envs {
            let key = format!("tree.{tree}.env.{k}");
            // 実効値: シェル環境が勝つ(§10.8)。注入は「未設定のときだけ」
            match std::env::var(k).ok().filter(|s| !s.is_empty()) {
                Some(shell) => println!("  {key:width$}  {shell}   (シェル環境が優先。設定は {v})"),
                None => println!("  {key:width$}  {v}   (設定ファイル)"),
            }
        }
    }
    if !secs.is_empty() {
        println!();
        for (k, v) in &secs {
            let key = format!("tree.{tree}.secret.{k}");
            // 宣言は参照のまま。解決しない(金庫に触らない)
            println!("  {key:width$}  {v}   (宣言。値は haj secret get {k})");
        }
    }
    show_effective_env(tree, installed, effective_env);
}

/// 全景の後半: 各コマンドの `--haj-env` の節連結(コマンドの既定値まで含む)。
/// 設定ファイルの写しではなく**実態**なので、tree設定・シェル環境の出所注記が
/// そのまま付いてくる(§10.8 の中継注記)。
fn show_effective_env(tree: &str, installed: bool, effective_env: Option<&str>) {
    println!();
    match effective_env {
        Some(eff) => {
            println!("実効 env (各コマンドの --haj-env。コマンドの既定値まで含む):");
            for line in eff.lines() {
                println!("  {line}");
            }
            println!();
            println!("(--env-file に渡せる素の形式は: haj env {tree})");
        }
        None if installed => {
            println!("実効 env: (--haj-env に応答するコマンドがありません)");
        }
        None => {
            println!("実効 env: (未インストールのため取得できません)");
        }
    }
}

/// `key = value` を並べただけの形式。`#` から行末はコメント。
/// **行末の `\` は継続**(シェルや git config と同じ)。継続行は空白1つで繋がる。
///
/// `.haj/config` と共用する。値は前後の空白と引用符を落とすだけで、
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
