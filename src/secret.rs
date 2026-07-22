//! `haj secret` — 宣言を引く(SPEC §10.9)。読みだけ。
//!
//! サブコマンドが金庫を直接読むと、接続と認証の知識がツリーごとに複製される。
//! `haj secret` はその口を一つにする — ただし解決するのは**宣言表(§10.8)に
//! ある参照だけ**。KEY で引き、参照は受けない。何を読めるかは宣言表が決める
//! (capability)— コードには物理パスも `store://` も書かれず、宣言表を迂回する
//! 口が無い。
//!
//! 宣言域は文脈で決まる(相補 — §10.8): ツリー文脈(`HAJ_TREE`)では
//! `tree.<名前>.secret.*` だけ、ツリーの外(人間のシェル・個人/共通コマンド)では
//! `user.secret.*` だけ。**誰の文脈かが、引ける目録を決める。**
//!
//! 所有の規律: **secret = 読む(他所の物も含む)、store = 読み書き(自分の物
//! だけ)**。書きたい秘密は自分の store(§10.10)に置く。

use std::path::PathBuf;

use crate::secrets::Delivery;

const USAGE: &str = "\
使い方: haj secret get <KEY>                       宣言を解決して値を stdout へ
        haj secret file <KEY>                      宣言を解決してファイルに実体化し、パスを stdout へ
        haj secret list  [--tree <インストール名>]  宣言の一覧 (KEY=<参照>。値は解決しない)
        haj secret check [--tree <インストール名>]  宣言と受け渡しの検証 (金庫に触らない)
宣言域は文脈で決まる: ツリーの中は tree.<HAJ_TREE>.secret.*、外は user.secret.*";

pub fn run(args: &[String], deliveries: &[Delivery]) -> ! {
    match args.split_first().map(|(a, r)| (a.as_str(), r)) {
        Some(("get", r)) => get_or_file(r, false),
        Some(("file", r)) => get_or_file(r, true),
        Some(("list", r)) => list(r),
        Some(("check", r)) => check(deliveries, r),
        _ => die(USAGE),
    }
}

/// 宣言域(SPEC §10.8)。ツリー文脈なら `tree.<名前>.secret.*`、外なら
/// `user.secret.*`。**相補** — 片方の文脈からもう片方の目録には届かない。
enum Scope {
    Tree(String),
    User,
}

impl Scope {
    /// `tree.<名前>.secret` / `user.secret` — 表示とエラーメッセージ用。
    fn label(&self) -> String {
        match self {
            Scope::Tree(t) => format!("tree.{t}.secret"),
            Scope::User => "user.secret".to_string(),
        }
    }

    /// store:// の解決文脈。user 域には無い(ユーザーに store の名前空間は無い)。
    fn tree(&self) -> Option<&str> {
        match self {
            Scope::Tree(t) => Some(t),
            Scope::User => None,
        }
    }

    /// 宣言表。**ユーザー設定からだけ**読む(§10.8 — 権威の規則は tree.* と同じ)。
    fn declarations(&self) -> Vec<(String, String)> {
        let cfg = crate::config::Config::load();
        match self {
            Scope::Tree(t) => cfg.tree_entries(t, "secret"),
            Scope::User => cfg.user_secret_entries(),
        }
    }
}

/// get / file の文脈: 環境の `HAJ_TREE` だけで決まる(値に触る操作 — §10.9)。
fn scope_ctx() -> Scope {
    match std::env::var("HAJ_TREE").ok().filter(|t| !t.is_empty()) {
        Some(t) => Scope::Tree(t),
        None => Scope::User,
    }
}

/// list / check の文脈: 人手用の `--tree <インストール名>` > 環境の `HAJ_TREE` >
/// user 域(SPEC §10.9 — その場の明示が常に勝つ)。get / file には無い —
/// 値に触る操作の対象切り替えを argv で気軽にさせない(取り違えたツリーの秘密を
/// 読む事故)。list / check は金庫に触らない読み取りメタ情報だから、この口が許される。
fn scope_ctx_args(args: &[String]) -> Scope {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--tree" {
            let Some(t) = it.next().filter(|t| !t.is_empty()) else {
                die("--tree には値が要ります: --tree <インストール名> (一覧: haj tree list)");
            };
            return Scope::Tree(t.clone());
        }
    }
    scope_ctx()
}

/// `haj secret get <KEY>`(値を stdout へ)/ `haj secret file <KEY>`(ファイルに
/// 実体化してパスを stdout へ)。宣言解決の規則は同一(§10.9)。
fn get_or_file(args: &[String], as_file: bool) -> ! {
    let verb = if as_file { "file" } else { "get" };
    if args.iter().any(|a| a == "--tree") {
        // capability の壁(SPEC §10.9): 値に触る操作の対象は文脈だけで決まる
        die(&format!(
            "haj secret {verb} に --tree はありません — 値に触る操作の対象は文脈 (HAJ_TREE) だけで決まります。\n  \
             人手なら HAJ_TREE=<インストール名> を明示して実行する (SPEC §10.9 / §10.10)"
        ));
    }
    let Some(key) = args.first() else {
        die(USAGE);
    };
    let scope = scope_ctx();
    let value = resolve_declared(&scope, key);
    if !as_file {
        // 値そのもの+改行1つ($(...) が改行を落とす)。§10.4 と同じ末尾規則
        println!("{value}");
        std::process::exit(0);
    }

    // ファイルに実体化(SPEC §10.9)。同じ KEY は呼ぶたび上書き。
    // 掃除 API は無い — $XDG_RUNTIME_DIR はセッション終了で消える(ssh-agent の
    // ソケットと同じ寿命観)。
    if !is_env_name(key) {
        die(&format!(
            "{key}: ファイル名にできません (KEY は英数字と _ のみ)"
        ));
    }
    let dir = secret_files_dir().unwrap_or_else(|e| die(&e));
    let path = dir.join(key);
    if let Err(e) = write_secret_file(&path, &value) {
        die(&format!("{} に書けません: {e}", path.display()));
    }
    println!("{}", path.display());
    std::process::exit(0);
}

/// 宣言表から KEY を引いて解決する(get / file 共通)。
/// 宣言に無い KEY・平文の宣言・user 域の store:// は、それぞれ案内して止まる。
fn resolve_declared(scope: &Scope, key: &str) -> String {
    let label = scope.label();
    let decls = scope.declarations();
    let Some((_, reference)) = decls.iter().find(|(k, _)| k == key) else {
        // 宣言に無い KEY はエラー(capability)。宣言済みを列挙して案内する。
        let listed = if decls.is_empty() {
            format!("(宣言はありません。~/.config/haj/config に {label}.{key} = <参照> を書く)")
        } else {
            let names: Vec<&str> = decls.iter().map(|(k, _)| k.as_str()).collect();
            format!("宣言済み: {}", names.join(", "))
        };
        die(&format!(
            "{key} は宣言されていません ({label}.{key})\n  {listed}"
        ));
    };
    if !crate::secrets::is_reference(reference) {
        die(&plaintext_err(scope, key));
    }
    if scope.tree().is_none() && reference.starts_with("store://") {
        // store:// はツリーの名前空間の参照(§10.7)。user 域には名前空間が無い。
        die(&format!(
            "{label}.{key}: store:// はツリーの名前空間の参照なので user 域では使えません。\n  \
             物理参照 (vault:// 等) を書いてください"
        ));
    }
    match crate::secrets::expand(reference, false, scope.tree()) {
        Ok(v) => v.unwrap_or_else(|| reference.clone()),
        Err(e) => die(&format!("{label}.{key}: {e}")),
    }
}

/// 平文の宣言はエラー — 秘密の平文を設定ファイルに書かせない(§10.8)。
fn plaintext_err(scope: &Scope, key: &str) -> String {
    let label = scope.label();
    let plain_hint = match scope {
        Scope::Tree(t) => format!("平文の設定なら tree.{t}.env.{key} に"),
        Scope::User => "平文はそもそも宣言の仕事ではない".to_string(),
    };
    format!(
        "{label}.{key}: 参照ではありません。\n  \
         秘密の平文は設定ファイルに書かない — {plain_hint}"
    )
}

/// 秘密ファイルの置き場: `$XDG_RUNTIME_DIR/haj/secret-files/`(tmpfs・0700・
/// ユーザー専有。ログアウトで消える)。無い環境は `$TMPDIR/haj-<uid>/secret-files/`
/// (0700。寿命は OS の tmp 掃除に従う)。
fn secret_files_dir() -> Result<PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;
    let base = match std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        Some(runtime) => runtime.join("haj"),
        None => {
            extern "C" {
                fn getuid() -> u32;
            }
            let uid = unsafe { getuid() };
            std::env::temp_dir().join(format!("haj-{uid}"))
        }
    };
    let dir = base.join("secret-files");
    for d in [&base, &dir] {
        std::fs::create_dir_all(d).map_err(|e| format!("{} を作れません: {e}", d.display()))?;
        std::fs::set_permissions(d, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("{} の権限を設定できません: {e}", d.display()))?;
    }
    Ok(dir)
}

/// 0600 で書く(上書き)。既存ファイルでもモードを強制する。
fn write_secret_file(path: &std::path::Path, content: &str) -> Result<(), String> {
    use std::io::Write as _;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| e.to_string())?;
    f.write_all(content.as_bytes()).map_err(|e| e.to_string())?;
    f.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())
}

/// KEY として妥当な字面(英数字と `_`、先頭は数字でない)。file のファイル名に使う。
fn is_env_name(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with(|c: char| c.is_ascii_digit())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn list(args: &[String]) -> ! {
    let scope = scope_ctx_args(args);
    let label = scope.label();
    let decls = scope.declarations();
    if decls.is_empty() {
        println!("宣言はありません ({label}.*)。");
        println!("  ~/.config/haj/config に {label}.<KEY> = <参照> を書く");
        std::process::exit(0);
    }
    // 参照は秘密ではない(§10.6)。値は解決しない。
    for (k, v) in decls {
        println!("{k}={v}");
    }
    std::process::exit(0);
}

/// `haj secret check` — 何が渡り、何が宣言されているのかを**解決せずに**確かめる
/// (SPEC §10.6)。金庫に問い合わせないので、ログインもタッチ認証も起きない。
fn check(deliveries: &[Delivery], args: &[String]) -> ! {
    let mut failed = false;
    let mut printed = false;
    // 対象は --tree の明示 > 環境の HAJ_TREE > user 域(§10.9)。受け渡しの
    // 注記と宣言の検証の両方が、同じ1つの対象に対して行われる。
    let scope = scope_ctx_args(args);

    // 受け渡しフラグの事前確認(旧 haj secrets --check)
    if !deliveries.is_empty() {
        println!(" 実行時に渡るもの (値は解決していません):");
        for d in deliveries {
            match d.plan() {
                Ok(rows) => {
                    for (kind, name, value) in rows {
                        let mark = if crate::secrets::is_reference(&value) {
                            "→"
                        } else {
                            " "
                        };
                        let note = value
                            .strip_prefix("store://")
                            .map(|rest| crate::store::check_note(rest, scope.tree()))
                            .unwrap_or_default();
                        println!("   {kind:10}  {name:20}  {mark} {value}{note}");
                    }
                }
                Err(e) => {
                    eprintln!("haj: {e}");
                    std::process::exit(1);
                }
            }
        }
        println!("\n (→ が付いたものが展開されます。他は平文としてそのまま渡ります)");
        printed = true;
    }

    // 宣言の検証。写像は手元の設定だけで決まる。
    let label = scope.label();
    let decls = scope.declarations();
    if printed {
        println!();
    }
    if decls.is_empty() {
        println!(" 宣言 ({label}.*): ありません");
    } else {
        println!(" 宣言 ({label}.*):");
        let width = decls.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
        for (k, v) in &decls {
            if !crate::secrets::is_reference(v) {
                println!("   {k:width$}  ✗ 参照ではありません (秘密の平文を設定に書かない)");
                failed = true;
            } else if scope.tree().is_none() && v.starts_with("store://") {
                println!("   {k:width$}  ✗ store:// は user 域では使えません (物理参照を書く)");
                failed = true;
            } else {
                let note = v
                    .strip_prefix("store://")
                    .map(|rest| crate::store::check_note(rest, scope.tree()))
                    .unwrap_or_default();
                println!("   {k:width$}  → {v}{note}");
            }
        }
    }
    std::process::exit(if failed { 1 } else { 0 });
}

/// 補完(builtin::complete から呼ばれる)。`get` / `file` には宣言済みの KEY —
/// 目録は手元の設定だけで列挙できる(金庫には触らない。SPEC §10.9)。
pub fn complete(words: &[String]) -> Vec<String> {
    match words.first().map(String::as_str) {
        None => ["get", "file", "list", "check"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        Some("get" | "file") if words.len() == 1 => scope_ctx()
            .declarations()
            .into_iter()
            .map(|(k, _)| k)
            .collect(),
        // 人手用の --tree(§10.9)。値はインストール済みツリー名(手元の列挙のみ)
        Some("list" | "check") if words.len() == 1 => vec!["--tree".to_string()],
        Some("list" | "check") if words.last().map(String::as_str) == Some("--tree") => {
            crate::tree::installed()
                .into_iter()
                .map(|(n, _)| n)
                .collect()
        }
        _ => Vec::new(),
    }
}

fn die(msg: &str) -> ! {
    eprintln!("haj: {msg}");
    std::process::exit(1);
}
