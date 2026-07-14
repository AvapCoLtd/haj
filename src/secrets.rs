//! シークレット参照の展開(SPEC.md §10)。
//!
//! サブコマンドに渡す値には、シークレットの実体ではなく参照
//! (`op://...`、`vault://...`、vault-agent template の展開式)を書ける。
//! コアは exec の直前に参照を解決し、展開済みの値だけを子プロセスに渡す。
//!
//! 書式は発明しない。op は `op inject` に丸ごと委譲し、vault は vault-agent
//! template の正準形をそのまま受ける。既存のテンプレートからコピペで移せる。
//!
//! 依存クレートは増やさない。op / vault は CLI を子プロセスで呼び、
//! env / file は stdlib だけで解決する。

use std::env;
use std::io::Write;
use std::process::{Command as Proc, Stdio};
use std::sync::OnceLock;

/// vault サーバの既定。環境に VAULT_ADDR / BAO_ADDR があればそちらを尊重する。
pub const DEFAULT_VAULT_ADDR: &str = "https://vault.avap.plus";
/// 未ログイン時に自動実行する `login` の既定引数。`off` で無効化。
pub const DEFAULT_VAULT_LOGIN: &str = "-method=oidc -path=id-avap-keycloak";

/// 展開は明示のオプトイン。clone した直後のリポジトリで勝手に金庫が開く、
/// という事故を防ぐため、既定では何もしない。
pub fn enabled() -> bool {
    env::var("HAJ_SECRETS").map(|v| v == "1").unwrap_or(false)
}

/// 値が参照なら展開して Some を、参照でなければ None を返す(触らない)。
/// 解決に失敗したら Err。呼び出し側は本体を実行せずに止まること(fail-fast)。
pub fn expand(value: &str) -> Result<Option<String>, String> {
    // 値全体が参照のものを先に見る。op だけは inject の意味論(埋め込みも展開)に
    // 従うため「含まれていれば」で拾う。順序が入れ替わると、vault:// の中に
    // op:// を含むような値の解釈が変わってしまう。
    if let Some(rest) = value.strip_prefix("vault://") {
        return vault_uri(rest).map(Some);
    }
    if value.starts_with("{{") && value.ends_with("}}") {
        return vault_template(value).map(Some);
    }
    if let Some(var) = value.strip_prefix("env://") {
        // 再帰はしない(1段だけ)。参照が参照を指す迷路を作らせない。
        return match env::var(var) {
            Ok(v) => Ok(Some(v)),
            Err(_) => Err(format!("env://{var}: 環境変数 {var} がありません")),
        };
    }
    if let Some(path) = value.strip_prefix("file://") {
        return match std::fs::read_to_string(path) {
            Ok(s) => Ok(Some(trim_newline(s))),
            Err(e) => Err(format!("file://{path}: 読めません: {e}")),
        };
    }
    if value.contains("op://") {
        return op_inject(value).map(Some);
    }
    Ok(None)
}

/// dry-run(`haj secrets`)用。解決せずに「展開対象か」だけ答える。
pub fn is_reference(value: &str) -> bool {
    value.starts_with("vault://")
        || (value.starts_with("{{") && value.ends_with("}}"))
        || value.starts_with("env://")
        || value.starts_with("file://")
        || value.contains("op://")
}

/// `vault://<パス>/<フィールド>` — 最後のセグメントがフィールド、残りがパス。
/// パスの規約は template 形と同じ(KV v2 は `/data/` 入り)。
fn vault_uri(rest: &str) -> Result<String, String> {
    let segs: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    if segs.len() < 2 {
        return Err(format!(
            "vault://{rest}: パスとフィールドが要ります (vault://<パス>/<フィールド>)"
        ));
    }
    let field = segs[segs.len() - 1];
    vault_fetch(&segs[..segs.len() - 1], field)
}

/// vault-agent template の正準形だけを解釈する。
///
///   {{ with secret "<パス>" }}{{ .Data.data.<フィールド> }}{{ end }}
///
/// それ以外の式(printf を含むもの等)は解釈しない。テンプレートエンジンを
/// 抱え込むことになるし、「どこまで動くのか」が誰にも分からなくなる。
fn vault_template(value: &str) -> Result<String, String> {
    const CANON: &str =
        "vault template は正準形のみ対応です: {{ with secret \"<パス>\" }}{{ .Data.data.<フィールド> }}{{ end }}";

    // {{ ... }} のブロックを取り出す。ブロック間に文字があれば正準形ではない。
    let mut blocks: Vec<&str> = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find("{{") {
        if !rest[..start].trim().is_empty() {
            return Err(CANON.to_string());
        }
        let Some(end) = rest[start..].find("}}") else {
            return Err(CANON.to_string());
        };
        blocks.push(rest[start + 2..start + end].trim());
        rest = &rest[start + end + 2..];
    }
    if !rest.trim().is_empty() || blocks.len() != 3 || blocks[2] != "end" {
        return Err(CANON.to_string());
    }

    let path = blocks[0]
        .strip_prefix("with secret")
        .map(str::trim)
        .and_then(|q| q.strip_prefix('"'))
        .and_then(|q| q.strip_suffix('"'))
        .ok_or(CANON)?;
    let field = blocks[1].strip_prefix(".Data.data.").ok_or(CANON)?.trim();
    if path.is_empty() || field.is_empty() || field.contains(char::is_whitespace) {
        return Err(CANON.to_string());
    }

    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    vault_fetch(&segs, field)
}

/// `vault kv get` で1フィールドを取る。パスの2セグメント目が `data` なら
/// KV v2 の API パス(template の書き方)とみなし、mount と相対パスに読み替える。
fn vault_fetch(path: &[&str], field: &str) -> Result<String, String> {
    let cli = cli_for("HAJ_VAULT_CMD", "vault_cmd", "bao");
    ensure_vault_login(&cli)?;
    let mut proc = vault_proc(&cli);
    proc.args(["kv", "get", &format!("-field={field}")]);
    if path.len() >= 3 && path[1] == "data" {
        proc.arg(format!("-mount={}", path[0]));
        proc.arg(path[2..].join("/"));
    } else {
        proc.arg(path.join("/"));
    }
    run(proc, &cli, None)
}

/// vault CLI のプロセスを作る。サーバは、環境に VAULT_ADDR / BAO_ADDR が
/// あればそちらを尊重し、無ければ設定 `vault_addr`(既定 vault.avap.plus)を
/// 両方の名前で渡す(bao は BAO_ADDR を先に見る)。
fn vault_proc(cli: &str) -> Proc {
    let mut proc = Proc::new(cli);
    let has_addr = ["BAO_ADDR", "VAULT_ADDR"]
        .iter()
        .any(|k| env::var(k).map(|v| !v.is_empty()).unwrap_or(false));
    if !has_addr {
        let (addr, _) =
            crate::config::Config::load().get("VAULT_ADDR", "vault_addr", DEFAULT_VAULT_ADDR);
        proc.env("VAULT_ADDR", &addr).env("BAO_ADDR", &addr);
    }
    proc
}

/// vault のログイン状態はプロセスで一度だけ確かめる。参照が何個あっても
/// `token lookup` と `login` が二度走らないように。
static VAULT_LOGIN: OnceLock<Result<(), String>> = OnceLock::new();

/// 未ログインなら、設定 `vault_login` の引数で `login` を実行してから解決に進む
/// (SPEC §10.4)。既定は avap の OIDC(`-method=oidc -path=id-avap-keycloak`)。
/// `off` で無効化 — そのときは解決が vault 自身のエラーで fail-fast する。
///
/// CI は VAULT_TOKEN 等で認証済みの前提(`token lookup` が通る)なので login は
/// 走らない。認証しない CI で vault 参照を使うなら `HAJ_VAULT_LOGIN=off` を置くこと
/// (OIDC はブラウザと人を待つ)。
fn ensure_vault_login(cli: &str) -> Result<(), String> {
    VAULT_LOGIN
        .get_or_init(|| {
            let logged_in = vault_proc(cli)
                .args(["token", "lookup"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if logged_in {
                return Ok(());
            }
            let (args, _) = crate::config::Config::load().get(
                "HAJ_VAULT_LOGIN",
                "vault_login",
                DEFAULT_VAULT_LOGIN,
            );
            if args == "off" {
                return Ok(()); // 自動ログインは明示的に無効化されている
            }
            let args: Vec<&str> = args.split_whitespace().collect();
            eprintln!(
                "haj: vault にログインします: {cli} login {}",
                args.join(" ")
            );
            // 端末を継ぐ。OIDC はブラウザと人を待つので、ここにタイムアウトは無い。
            let status = vault_proc(cli)
                .arg("login")
                .args(&args)
                .status()
                .map_err(|e| format!("{cli} login を実行できません: {e}"))?;
            if status.success() {
                Ok(())
            } else {
                Err(format!("{cli} login が失敗しました"))
            }
        })
        .clone()
}

/// op は書式を解釈せず、値ごと `op inject` に渡す。埋め込みの展開も
/// 意味論もすべて inject に従う。
fn op_inject(value: &str) -> Result<String, String> {
    let cli = cli_for("HAJ_OP_CMD", "op_cmd", "op");
    let mut proc = Proc::new(&cli);
    proc.arg("inject");
    run(proc, &cli, Some(value))
}

/// リゾルバCLIを実行して stdout を採る。stderr はそのまま流す(失敗の理由は
/// CLI自身が一番よく知っている)。タイムアウトは設けない — op のタッチ認証など、
/// 人を待つ場面が正当にある。規約フックの2秒とは別物。
fn run(mut proc: Proc, cli: &str, stdin: Option<&str>) -> Result<String, String> {
    proc.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(Stdio::piped());

    let mut child = proc.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("{cli} が見つかりません (HAJ_OP_CMD / HAJ_VAULT_CMD で差し替えられます)")
        } else {
            format!("{cli} を実行できません: {e}")
        }
    })?;

    if let Some(input) = stdin {
        let mut pipe = child.stdin.take().expect("stdin(piped) は必ず在る");
        pipe.write_all(input.as_bytes())
            .map_err(|e| format!("{cli} に値を渡せません: {e}"))?;
        // ここで drop して EOF を伝える。しないと inject が入力を待ち続ける。
    }

    let out = child
        .wait_with_output()
        .map_err(|e| format!("{cli} の結果を読めません: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{cli} が失敗しました (exit {})",
            exit_code(&out.status)
        ));
    }
    String::from_utf8(out.stdout)
        .map(trim_newline)
        .map_err(|_| format!("{cli} の出力が UTF-8 ではありません"))
}

/// `haj secrets` — 何が展開対象なのかを、解決せずに確かめる(SPEC §10.6)。
/// 参照の対象(パス)は出すが、値は解決しない。金庫に問い合わせない。
pub fn dry_run() -> ! {
    if enabled() {
        println!("HAJ_SECRETS=1 (展開は有効)");
    } else {
        println!("HAJ_SECRETS が設定されていません (展開は無効。参照はただの文字列として渡ります)");
    }

    let mut refs: Vec<(String, String)> = env::vars().filter(|(_, v)| is_reference(v)).collect();
    refs.sort();

    if refs.is_empty() {
        println!("\n 展開対象の環境変数はありません。");
    } else {
        println!("\n 環境変数:");
        let width = refs.iter().map(|(k, _)| k.len()).max().unwrap_or(8);
        for (k, v) in &refs {
            println!("   {k:width$}  {v}");
        }
    }
    std::process::exit(0);
}

/// リゾルバCLIの決定。環境変数 > 設定ファイル > 既定値(SPEC §8.3)。
/// avap は `vault_cmd = bao` を設定ファイルに書いて差し替える。
fn cli_for(env_key: &str, file_key: &str, default: &str) -> String {
    crate::config::Config::load()
        .get(env_key, file_key, default)
        .0
}

/// 末尾の改行1つを落とす。CLI や credential ファイルが付けるもの。
fn trim_newline(mut s: String) -> String {
    if s.ends_with('\n') {
        s.pop();
        if s.ends_with('\r') {
            s.pop();
        }
    }
    s
}

fn exit_code(status: &std::process::ExitStatus) -> String {
    status
        .code()
        .map(|c| c.to_string())
        .unwrap_or_else(|| "シグナル".to_string())
}
