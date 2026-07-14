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

/// vault サーバの既定。空 = 注入しない(環境や CLI 自身の設定に任せる)。
/// 環境に VAULT_ADDR / BAO_ADDR があればそちらを尊重する。
pub const DEFAULT_VAULT_ADDR: &str = "";
/// 未ログイン時に自動実行する `login` の引数。既定は off(勝手にログインしない)。
/// OpenBao/Vault の OIDC を使うなら設定に書く: secrets.vault_login = -method=oidc
pub const DEFAULT_VAULT_LOGIN: &str = "off";
/// vault 参照の解決に使う CLI。OpenBao なら設定で secrets.vault_cmd = bao。
pub const DEFAULT_VAULT_CMD: &str = "vault";

/// 展開は明示のオプトイン。clone した直後のリポジトリで勝手に金庫が開く、
/// という事故を防ぐため、既定では何もしない。
pub fn enabled() -> bool {
    env::var("HAJ_SECRETS").map(|v| v == "1").unwrap_or(false)
}

/// 値が参照なら展開して Some を、参照でなければ None を返す(触らない)。
/// 解決に失敗したら Err。呼び出し側は本体を実行せずに止まること(fail-fast)。
///
/// `embedded_op` は「op:// を**含む**だけで inject に回す」かどうか。
/// argv のように**人が明示的に書いた**値だけ true にする。環境変数の走査で
/// これをやると、参照をたまたま文中に含む変数(GitLab CI が置く
/// CI_MERGE_REQUEST_DESCRIPTION に op:// の例文が入っている、等)を解決しようと
/// して、無関係な理由で全体が止まる。
pub fn expand(value: &str, embedded_op: bool) -> Result<Option<String>, String> {
    // 値全体が参照のものを先に見る。順序が入れ替わると、vault:// の中に
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
    if value.starts_with("op://") || (embedded_op && value.contains("op://")) {
        return op_inject(value).map(Some);
    }
    Ok(None)
}

/// dry-run(`haj secrets`)用。解決せずに「展開対象か」だけ答える。
/// 環境変数の走査と同じ規則(op も値全体のときだけ)。
pub fn is_reference(value: &str) -> bool {
    value.starts_with("vault://")
        || (value.starts_with("{{") && value.ends_with("}}"))
        || value.starts_with("env://")
        || value.starts_with("file://")
        || value.starts_with("op://")
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

const CANON: &str =
    "vault template は正準形のみ対応です: {{ with secret \"<パス>\" }}{{ .Data.data.<フィールド> }}{{ end }}";

/// vault-agent template の正準形だけを解釈する。
///
///   {{ with secret "<パス>" }}{{ .Data.data.<フィールド> }}{{ end }}
///
/// それ以外の式(printf を含むもの等)は解釈しない。テンプレートエンジンを
/// 抱え込むことになるし、「どこまで動くのか」が誰にも分からなくなる。
fn vault_template(value: &str) -> Result<String, String> {
    let t = value.trim();
    match parse_canonical(t) {
        Some((path, field, consumed)) if t[consumed..].trim().is_empty() => {
            let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
            vault_fetch(&segs, &field)
        }
        _ => Err(CANON.to_string()),
    }
}

/// 正準形トリプルを先頭から1つ読む。読めたら (パス, フィールド, 消費したバイト数)。
/// テンプレート描画(--secretfile)が同じ規則で文中のブロックを置換できるように、
/// 「値全体」の判定は呼び出し側に委ねる。
fn parse_canonical(s: &str) -> Option<(String, String, usize)> {
    let (b1, r1) = take_block(s)?;
    let (b2, r2) = take_block(r1)?;
    let (b3, r3) = take_block(r2)?;
    if b3 != "end" {
        return None;
    }
    let path = b1
        .strip_prefix("with secret")?
        .trim()
        .strip_prefix('"')?
        .strip_suffix('"')?;
    let field = b2.strip_prefix(".Data.data.")?.trim();
    if path.is_empty() || field.is_empty() || field.contains(char::is_whitespace) {
        return None;
    }
    Some((path.to_string(), field.to_string(), s.len() - r3.len()))
}

/// 先頭の `{{ ... }}` を1つ取る(手前の空白は読み飛ばす)。(中身, 残り) を返す。
fn take_block(s: &str) -> Option<(&str, &str)> {
    let inner = s.trim_start().strip_prefix("{{")?;
    let end = inner.find("}}")?;
    Some((inner[..end].trim(), &inner[end + 2..]))
}

/// 明示的な受け渡し(SPEC §10.7)。サブコマンド名の**前**のグローバルフラグ。
/// フラグを打ったこと自体が同意なので、HAJ_SECRETS のゲートは通らない。
pub enum Delivery {
    /// `--secret <名前>=<値>` — 展開して環境変数で渡す。参照でなければ平文のまま
    Secret { name: String, value: String },
    /// `--env <ファイル>` — `key = value` を読み、値全体規則で展開して渡す
    EnvFile(String),
    /// `--secretfile <出力>=<テンプレート>` — 描画して 0600 で書く
    SecretFile { out: String, template: String },
}

impl Delivery {
    pub fn parse(flag: &str, arg: &str) -> Result<Delivery, String> {
        let split = || {
            arg.split_once('=')
                .filter(|(k, v)| !k.is_empty() && !v.is_empty())
        };
        match flag {
            "--secret" => split()
                .map(|(k, v)| Delivery::Secret {
                    name: k.to_string(),
                    value: v.to_string(),
                })
                .ok_or_else(|| format!("--secret は <名前>=<値> で指定してください: {arg}")),
            "--env" => Ok(Delivery::EnvFile(arg.to_string())),
            "--secretfile" => split()
                .map(|(o, t)| Delivery::SecretFile {
                    out: o.to_string(),
                    template: t.to_string(),
                })
                .ok_or_else(|| {
                    format!("--secretfile は <出力>=<テンプレート> で指定してください: {arg}")
                }),
            other => Err(format!("不明なフラグです: {other}")),
        }
    }

    /// 解決して proc に適用する。書いた順に呼ぶこと(同名は後勝ち)。
    /// 失敗したら Err — 呼び出し側は本体を実行せずに止まる(fail-fast)。
    pub fn apply(&self, proc: &mut Proc) -> Result<(), String> {
        match self {
            Delivery::Secret { name, value } => {
                // 明示なので op の埋め込みも展開する(argv と同じ規則)
                let v = expand(value, true)
                    .map_err(|e| format!("--secret {name}: {e}"))?
                    .unwrap_or_else(|| value.clone());
                proc.env(name, v);
                Ok(())
            }
            Delivery::EnvFile(file) => {
                let content = std::fs::read_to_string(file)
                    .map_err(|e| format!("--env {file}: 読めません: {e}"))?;
                for (k, v) in crate::config::parse_kv(&content) {
                    // 値全体規則(環境変数の走査と同じ)。ファイルは中間層なので
                    // 埋め込みは解釈しない
                    let v = expand(&v, false)
                        .map_err(|e| format!("--env {file}: {k}: {e}"))?
                        .unwrap_or(v);
                    proc.env(k, v);
                }
                Ok(())
            }
            Delivery::SecretFile { out, template } => {
                let content = std::fs::read_to_string(template)
                    .map_err(|e| format!("--secretfile {template}: 読めません: {e}"))?;
                let rendered = render_template(&content)
                    .map_err(|e| format!("--secretfile {template}: {e}"))?;
                write_secret_file(out, &rendered)
                    .map_err(|e| format!("--secretfile {out}: 書けません: {e}"))
            }
        }
    }
}

/// `--secretfile` のテンプレート描画。vault の正準形ブロックを置換し、
/// op:// を含めばファイル全体を `op inject` に通す(SPEC §10.7)。
/// `vault://` 短縮形はテンプレート内では解釈しない(トークンの境界が曖昧になる)。
fn render_template(content: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = content;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        let Some((path, field, len)) = parse_canonical(tail) else {
            return Err(format!("テンプレート内の {CANON}"));
        };
        let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        out.push_str(&vault_fetch(&segs, &field)?);
        rest = &tail[len..];
    }
    out.push_str(rest);

    if out.contains("op://") {
        // run() は値向けに末尾の改行を1つ落とす。ファイルでは元の形を保つ。
        let had_newline = out.ends_with('\n');
        out = op_inject(&out)?;
        if had_newline && !out.ends_with('\n') {
            out.push('\n');
        }
    }
    Ok(out)
}

/// 全て解決できてから mode 0600 で書く。半端なファイルを残さない。
/// 書いたファイルは消さない — コアは exec(2) で自分を置き換えるため、
/// 実行後の後始末は構造的に不可能(SPEC §10.7)。
fn write_secret_file(path: &str, content: &str) -> Result<(), String> {
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
    // 既存ファイルを上書きした場合(mode は作成時にしか効かない)もモードを強制する
    f.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// `vault kv get` で1フィールドを取る。パスの2セグメント目が `data` なら
/// KV v2 の API パス(template の書き方)とみなし、mount と相対パスに読み替える。
fn vault_fetch(path: &[&str], field: &str) -> Result<String, String> {
    let cli = cli_for("HAJ_VAULT_CMD", "secrets.vault_cmd", DEFAULT_VAULT_CMD);
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
/// あればそちらを尊重し、無ければ設定 `secrets.vault_addr` を両方の名前で渡す
/// (bao は BAO_ADDR を先に見る)。空なら何も注入しない。
fn vault_proc(cli: &str) -> Proc {
    let mut proc = Proc::new(cli);
    let has_addr = ["BAO_ADDR", "VAULT_ADDR"]
        .iter()
        .any(|k| env::var(k).map(|v| !v.is_empty()).unwrap_or(false));
    if !has_addr {
        let (addr, _) = crate::config::Config::load().get(
            "VAULT_ADDR",
            "secrets.vault_addr",
            DEFAULT_VAULT_ADDR,
        );
        proc.env("VAULT_ADDR", &addr).env("BAO_ADDR", &addr);
    }
    proc
}

/// vault のログイン状態はプロセスで一度だけ確かめる。参照が何個あっても
/// `token lookup` と `login` が二度走らないように。
static VAULT_LOGIN: OnceLock<Result<(), String>> = OnceLock::new();

/// 未ログインなら、設定 `vault_login` の引数で `login` を実行してから解決に進む
/// (SPEC §10.4)。既定は `off`(勝手にログインしない)。設定すると有効になる。
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
                "secrets.vault_login",
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
    let cli = cli_for("HAJ_OP_CMD", "secrets.op_cmd", "op");
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
/// OpenBao なら `secrets.vault_cmd = bao` を設定ファイルに書いて差し替える。
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
