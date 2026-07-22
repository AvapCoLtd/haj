//! シークレットストア(SPEC §10.7 / §10.10)。
//!
//! ストアは、エンジン(金庫)の中に haj が確保する**ツリー・インスタンス専用の
//! 名前空間**。参照 `store://<論理パス>` は `<prefix>/trees/<HAJ_TREE>/<論理パス>` に
//! 展開される。ツリーは自分の名前を書かない — 明示形(`store://<ツリー名>/...`)は
//! 無く、ツリーをまたぐ参照は構文レベルで存在しない(またぎは物理参照で)。
//!
//! ストアの設定は表 `store.<名前>.{engine, prefix}` で、v1 で存在する行は予約された
//! `tree`(store:// が指す)だけ。エンジンは v1 では型名 vault を直接書く。接続・認証・
//! セッションは物理参照(§10.4)と同じ機構(`secrets.vault_*`)をそのまま使う —
//! ストアが足すのは名前空間の写像だけで、金庫との話し方は増えない。

use std::io::{Read, Write as _};
use std::process::{Command as Proc, Stdio};

pub const DEFAULT_ENGINE: &str = "vault";
/// `haj config` に見せる既定値の表記。実際の既定は実行ユーザー名で埋まる。
pub const DEFAULT_PREFIX_DOC: &str = "secret/data/users/<ユーザー名>";

const USAGE: &str = "\
使い方: haj store get <論理パス>            値を stdout へ
        haj store put [--force] <論理パス>  stdin から値を読んで書く
        haj store login                     エンジンにログインする
        haj store status                    ログイン状態と実効設定
store は常に自分の名前空間 (HAJ_TREE) を操作する: haj store put token";

/// 旧キー(0.31.0 の store.engine / store.prefix)が設定に残っていたら、
/// プロセスにつき1回だけ警告して無視する。互換エイリアスは設けない
/// (採用前の改名。SPEC 追記 0.32.0)。
fn warn_renamed_keys(cfg: &crate::config::Config) {
    static WARNED: std::sync::Once = std::sync::Once::new();
    let old: Vec<&str> = ["store.engine", "store.prefix"]
        .into_iter()
        .filter(|k| cfg.has_key(k))
        .collect();
    if old.is_empty() {
        return;
    }
    WARNED.call_once(|| {
        for k in old {
            eprintln!(
                "haj: warning: {k} は store.tree.{} に改名されました (無視します)",
                k.rsplit('.').next().unwrap()
            );
        }
    });
}

/// ストア `tree` のエンジン(v1 は型名 vault のみ)。
fn engine() -> String {
    let cfg = crate::config::Config::load();
    warn_renamed_keys(&cfg);
    cfg.get("HAJ_STORE_TREE_ENGINE", "store.tree.engine", DEFAULT_ENGINE)
        .0
}

/// 物理プレフィックス。既定は `secret/data/users/<実行ユーザー名>`。
/// 書式は物理参照(vault://)のパスと同じ(KV v2 の /data/ 入り)。
fn prefix() -> Result<String, String> {
    let cfg = crate::config::Config::load();
    warn_renamed_keys(&cfg);
    if let Some((v, _)) = cfg.get_opt("HAJ_STORE_TREE_PREFIX", "store.tree.prefix") {
        return Ok(v.trim_matches('/').to_string());
    }
    let user = std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .map_err(|_| {
            "実行ユーザー名が分かりません (USER / LOGNAME)。store.tree.prefix を設定してください"
                .to_string()
        })?;
    Ok(format!("secret/data/users/{user}"))
}

/// `store://<論理パス>` を物理パス(vault:// の rest 形)に写像する。
/// 最後のセグメントがフィールドなのは vault:// と同じ規則(写像先で解釈される)。
fn to_physical(logical: &str, tree: &str) -> Result<String, String> {
    let e = engine();
    if e != "vault" {
        return Err(format!(
            "store.tree.engine = {e} には対応していません (v1 は vault のみ)"
        ));
    }
    let segs: Vec<&str> = logical.split('/').filter(|s| !s.is_empty()).collect();
    if segs.is_empty() {
        return Err(format!(
            "store://{logical}: 論理パスが要ります (store://<論理パス>。最後のセグメントがフィールド)"
        ));
    }
    Ok(format!("{}/trees/{}/{}", prefix()?, tree, segs.join("/")))
}

/// ツリー文脈が無いときのエラー(SPEC §10.10 — 3状態の1つ目)。
fn no_context_err(rest: &str) -> String {
    format!(
        "store://{rest}: ツリーのコマンドの中でだけ使えます (HAJ_TREE が無い)。\n  \
         人手の点検・移行なら HAJ_TREE=<インストール名> を明示して実行する (SPEC §10.10)"
    )
}

/// 参照の解決(secrets::expand から呼ばれる)。`tree` は渡す相手の所属ツリー。
/// 失敗時は展開後の物理写像を添える — 名前空間で隠した物理は、失敗の瞬間には
/// 見えなければならない(§2.4)。
pub fn resolve(rest: &str, tree: Option<&str>) -> Result<String, String> {
    let Some(tree) = tree else {
        return Err(no_context_err(rest));
    };
    let physical = to_physical(rest, tree)?;
    crate::secrets::vault_uri(&physical)
        .map_err(|e| format!("store://{rest} (→ vault://{physical}): {e}"))
}

/// `haj config --tree` に見せる、このインスタンスの名前空間(写像先)。
/// 金庫には触らない(写像は手元の設定だけで決まる)。
pub fn namespace_display(tree: &str) -> String {
    match to_physical("<論理パス>", tree) {
        Ok(p) => format!("vault://{p}  (engine: {})", engine()),
        Err(e) => format!("({e})"),
    }
}

/// `haj secret check` 用の注記。金庫には触らない(写像は手元の設定だけで決まる)。
/// `tree` は check が解決した対象(`--tree` の明示 > 環境の `HAJ_TREE` — §10.9)。
pub fn check_note(rest: &str, tree: Option<&str>) -> String {
    match tree {
        Some(tree) => match to_physical(rest, tree) {
            Ok(p) => format!("  (→ vault://{p})"),
            Err(e) => format!("  ({e})"),
        },
        None => {
            let p = prefix().unwrap_or_else(|_| "<prefix>".to_string());
            format!("  (→ vault://{p}/trees/<HAJ_TREE>/{rest} — ツリー文脈で決まる)")
        }
    }
}

/// ツリーごとの設定注入(SPEC §10.8)。注入されるのは **`.env`(平文・無展開)だけ**。
/// `.secret` は宣言であり、注入という経路が無い — コマンドが `haj secret get` で
/// 引く(§10.9)。**その変数が未設定のときだけ**注入する — シェル環境が常に勝ち、
/// グローバルフラグはこの後に適用されるのでさらに勝つ
/// (フラグ > シェル環境 > tree設定 > コマンド既定値)。
pub fn inject_tree_env(proc: &mut Proc, tree: &str) {
    for (key, val) in crate::config::Config::load().tree_entries(tree, "env") {
        if std::env::var_os(&key).is_none() {
            proc.env(&key, &val);
        }
    }
}

/// `haj env` の出所の注記(SPEC §10.8)。注記するのは **`.env` の分だけ** —
/// 秘密の目録は env とは別物なので `haj secret list`(§10.9)が受け持つ。
/// コメントは `--env-file` で読み飛ばされるので、出力の互換は保たれる。
pub fn annotate_env(out: &str, tree: &str) -> String {
    let envs = crate::config::Config::load().tree_entries(tree, "env");
    if envs.is_empty() {
        return out.to_string();
    }
    out.lines()
        .map(|line| {
            let t = line.trim();
            let Some((k, _)) = t.split_once('=') else {
                return line.to_string();
            };
            let k = k.trim();
            if t.is_empty() || t.starts_with('#') || k.is_empty() {
                return line.to_string();
            }
            let in_tree = envs.iter().any(|(e, _)| e == k);
            if std::env::var_os(k).is_some() {
                // シェル環境が勝っている(tree設定がある鍵にだけ注記する)
                if in_tree {
                    return format!("{line}   # シェル環境 (tree設定より優先)");
                }
                return line.to_string();
            }
            if let Some((_, v)) = envs.iter().find(|(e, _)| e == k) {
                return format!("{k}={v}   # tree設定 (env)");
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---- haj store(組み込み。SPEC §10.10) ----

pub fn run(args: &[String]) -> ! {
    match args.split_first().map(|(a, r)| (a.as_str(), r)) {
        Some(("get", r)) => get(r),
        Some(("put", r)) => put(r),
        Some(("login", _)) => login(),
        Some(("status", _)) => status(),
        _ => die(USAGE),
    }
}

/// 論理パスを KV の (パスのセグメント列, フィールド, 物理パス) に落とす。
/// **store は常に自分の名前空間**(自分の環境の HAJ_TREE)を操作する(SPEC §10.10)。
/// `store://` 前置きは読み飛ばす — 設定から**データとして**受け取った参照
/// (`TOKEN_OUTPUT=store://token`)をそのまま渡せる。物理参照(vault://)は
/// 受けない — 点検・横断・移行はツリー文脈の外で・人の明示で(§10.10)。
fn parse_logical(arg: &str) -> (Vec<String>, String, String) {
    if arg.strip_prefix("vault://").is_some() {
        die(
            "haj store は物理参照 (vault://) を受けません — store は自分の名前空間だけを操作します。\n  \
             点検 (読み): haj --secret V=vault://... sh 'printf \"%s\\n\" \"$V\"'\n  \
             移行 (書き): エンジンの CLI で (bao kv put ...)",
        );
    }
    let logical = arg.strip_prefix("store://").unwrap_or(arg);
    let Some(tree) = std::env::var("HAJ_TREE").ok().filter(|t| !t.is_empty()) else {
        die(&no_context_err(logical));
    };
    let physical = match to_physical(logical, &tree) {
        Ok(p) => p,
        Err(e) => die(&e),
    };
    let segs: Vec<String> = physical
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if segs.len() < 2 {
        die(&format!(
            "vault://{physical}: パスとフィールドが要ります (最後のセグメントがフィールド)"
        ));
    }
    let field = segs[segs.len() - 1].clone();
    (segs[..segs.len() - 1].to_vec(), field, physical)
}

/// KV v2 の API パス(/data/ 入り)なら -mount 形に読み替える(§10.4 と同じ規則)。
fn kv_target(path: &[String]) -> (Option<String>, String) {
    if path.len() >= 3 && path[1] == "data" {
        (Some(path[0].clone()), path[2..].join("/"))
    } else {
        (None, path.join("/"))
    }
}

fn kv_proc(cli: &str, sub: &str, mount: &Option<String>) -> Proc {
    let mut p = crate::secrets::vault_proc(cli);
    p.arg("kv").arg(sub);
    if let Some(m) = mount {
        p.arg(format!("-mount={m}"));
    }
    p
}

fn get(args: &[String]) -> ! {
    let Some(arg) = args.first() else {
        die(USAGE);
    };
    let (path, field, physical) = parse_logical(arg);
    let cli = crate::secrets::vault_cli();
    if let Err(e) = crate::secrets::ensure_vault_login(&cli) {
        die(&e);
    }
    let (mount, rel) = kv_target(&path);
    let mut p = kv_proc(&cli, "get", &mount);
    p.arg(format!("-field={field}")).arg(&rel);
    p.stdin(Stdio::null()).stdout(Stdio::piped());
    let out = match p.output() {
        Ok(o) => o,
        Err(e) => die(&format!("{cli} を実行できません: {e}")),
    };
    if !out.status.success() {
        // stderr はそのまま流れている(継いでいる)。物理写像を添える(§10.10)
        die(&format!(
            "{arg} → {cli} kv get {}-field={field} {rel} が失敗しました (vault://{physical})",
            mount.map(|m| format!("-mount={m} ")).unwrap_or_default()
        ));
    }
    let value = String::from_utf8_lossy(&out.stdout).to_string();
    // 値そのもの+改行1つ($(...) が改行を落とす)。末尾の改行の扱いは §10.4 と同じ
    println!("{}", crate::secrets::trim_newline(value));
    std::process::exit(0);
}

fn put(args: &[String]) -> ! {
    let force = args.iter().any(|a| a == "--force");
    let Some(arg) = args.iter().find(|a| !a.starts_with('-')) else {
        die(USAGE);
    };
    let (path, field, physical) = parse_logical(arg);
    let value = read_secret_stdin();

    let cli = crate::secrets::vault_cli();
    if let Err(e) = crate::secrets::ensure_vault_login(&cli) {
        die(&e);
    }
    let (mount, rel) = kv_target(&path);

    // オブジェクトとフィールドの有無を先に見る。
    //   オブジェクト無し → kv put(新規作成)
    //   オブジェクト有り → kv patch(他のフィールドを壊さない)。
    //     フィールドも有れば --force が無いかぎり拒否(上書きは明示 — §10.9)
    let object_exists = {
        let mut p = kv_proc(&cli, "get", &mount);
        p.arg(&rel)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        p.status().map(|s| s.success()).unwrap_or(false)
    };
    if object_exists && !force {
        let mut p = kv_proc(&cli, "get", &mount);
        p.arg(format!("-field={field}"))
            .arg(&rel)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if p.status().map(|s| s.success()).unwrap_or(false) {
            die(&format!(
                "既にフィールドがあります: vault://{physical}\n  上書きするなら: haj store put --force {arg}"
            ));
        }
    }

    let verb = if object_exists { "patch" } else { "put" };
    let mut p = kv_proc(&cli, verb, &mount);
    // 値は argv に置かない(`ps` に見える)。`<フィールド>=-` で stdin から渡す
    p.arg(&rel).arg(format!("{field}=-"));
    p.stdin(Stdio::piped()).stdout(Stdio::null());
    let mut child = match p.spawn() {
        Ok(c) => c,
        Err(e) => die(&format!("{cli} を実行できません: {e}")),
    };
    {
        let mut pipe = child.stdin.take().expect("stdin(piped) は必ず在る");
        if pipe.write_all(value.as_bytes()).is_err() {
            die(&format!("{cli} に値を渡せません"));
        }
        // drop で EOF
    }
    let st = child
        .wait()
        .unwrap_or_else(|e| die(&format!("{cli} の結果を読めません: {e}")));
    if !st.success() {
        die(&format!(
            "{arg} → {cli} kv {verb} {}{rel} {field}=- が失敗しました (vault://{physical})",
            mount.map(|m| format!("-mount={m} ")).unwrap_or_default()
        ));
    }
    eprintln!("haj: 書きました: vault://{physical}");
    std::process::exit(0);
}

/// stdin から値を読む。TTY ならエコーを切って1行、パイプなら全部。
/// 末尾の改行1つは落とす(`echo x | haj store put ...` が `x` を書く)。
fn read_secret_stdin() -> String {
    extern "C" {
        fn isatty(fd: i32) -> i32;
    }
    let tty = unsafe { isatty(0) } == 1;
    let mut buf = String::new();
    if tty {
        eprint!("値を入力 (エコーなし。Enter で確定): ");
        // エコー制御は stty に委譲する(op / bao と同じ CLI 委譲の流儀。
        // 無ければエコーされたまま進む — 機能は損なわない)
        let off = Proc::new("stty").arg("-echo").status();
        let mut line = String::new();
        let read = std::io::stdin().read_line(&mut line);
        if off.map(|s| s.success()).unwrap_or(false) {
            let _ = Proc::new("stty").arg("echo").status();
            eprintln!();
        }
        if read.is_err() {
            die("stdin を読めません");
        }
        buf = line;
    } else if std::io::stdin().read_to_string(&mut buf).is_err() {
        die("stdin を読めません");
    }
    let v = crate::secrets::trim_newline(buf);
    if v.is_empty() {
        die("値が空です (stdin から渡してください: ... | haj store put <論理パス>)");
    }
    v
}

fn login() -> ! {
    let cli = crate::secrets::vault_cli();
    // 明示の再認証も自動ログインと同じ連鎖(§10.4): cert 委譲 → OIDC。
    // token lookup は見ない — login は「いま認証し直す」ための動詞。
    match crate::secrets::try_cert_login(&cli) {
        Some(true) => {
            eprintln!("haj: cert 認証でログインしました");
            std::process::exit(0);
        }
        Some(false) => {} // 静かに次の段へ(try_cert_login が一行出している)
        None => {}        // cert 段は未設定
    }
    let (args, _) = crate::config::Config::load().get(
        "HAJ_VAULT_LOGIN",
        "secrets.vault_login",
        crate::secrets::DEFAULT_VAULT_LOGIN,
    );
    if args == "off" {
        die(&format!(
            "ログインの連鎖に使える段がありません。\n  \
             cert 委譲: secrets.vault_cert_login = <コマンド> (PIN/タッチのみ。ブラウザ不要)\n  \
             OIDC:      secrets.vault_login = -method=oidc\n  \
             を設定するか、{cli} login を直接実行"
        ));
    }
    let args: Vec<&str> = args.split_whitespace().collect();
    eprintln!("haj: {cli} login {}", args.join(" "));
    // 端末を継ぐ。OIDC はブラウザと人を待つので、タイムアウトは無い(§10.4)
    let st = crate::secrets::vault_proc(&cli)
        .arg("login")
        .args(&args)
        .status();
    match st {
        Ok(s) if s.success() => std::process::exit(0),
        Ok(_) => die(&format!("{cli} login が失敗しました")),
        Err(e) => die(&format!("{cli} login を実行できません: {e}")),
    }
}

fn status() -> ! {
    let cli = crate::secrets::vault_cli();
    let addr = ["BAO_ADDR", "VAULT_ADDR"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
        .or_else(|| {
            crate::config::Config::load()
                .get_opt("VAULT_ADDR", "secrets.vault_addr")
                .map(|(v, _)| v)
        })
        .unwrap_or_else(|| "(未設定)".to_string());
    println!("engine  {}", engine());
    println!("prefix  {}", prefix().unwrap_or_else(|e| format!("({e})")));
    println!("cli     {cli}");
    println!("addr    {addr}");
    // 自動ログインの連鎖(§10.4)。「login で再認証」の案内が連鎖を正しく説明する
    let cfg = crate::config::Config::load();
    let (cert, _) = cfg.get(
        "HAJ_VAULT_CERT_LOGIN",
        "secrets.vault_cert_login",
        crate::secrets::DEFAULT_VAULT_CERT_LOGIN,
    );
    let (oidc, _) = cfg.get(
        "HAJ_VAULT_LOGIN",
        "secrets.vault_login",
        crate::secrets::DEFAULT_VAULT_LOGIN,
    );
    let cert = if cert.is_empty() {
        "(無し)".to_string()
    } else {
        cert
    };
    println!("auto    token lookup → cert委譲: {cert} → login: {oidc}");
    match std::env::var("HAJ_TREE").ok().filter(|t| !t.is_empty()) {
        Some(t) => println!("tree    {t}  (論理パスは trees/{t}/ に写像)"),
        None => println!("tree    (無し — get / put はツリーのコマンドの中でだけ使える)"),
    }

    let logged_in = crate::secrets::vault_token_valid(&cli);
    if logged_in {
        println!("login   ログイン済み");
        std::process::exit(0);
    }
    println!("login   未ログイン (haj store login)");
    std::process::exit(1);
}

fn die(msg: &str) -> ! {
    eprintln!("haj: {msg}");
    std::process::exit(1);
}
