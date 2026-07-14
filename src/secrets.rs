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

const CANON: &str = "vault template を解釈できません。対応する形: \
{{ with secret \"<パス>\" }} … {{ .Data.data.<フィールド> }} … {{ end }} \
(ブロック内は地の文と複数フィールド可。空白制御 {{- -}} も可)";

/// vault-agent template の `with secret` ブロックを解釈する。
///
///   {{ with secret "<パス>" }} … {{ .Data.data.<フィールド> }} … {{ end }}
///
/// vault-agent の実テンプレートに合わせて、ブロックの中には地の文と複数の
/// フィールド参照を書け、Go template の空白制御(`{{-` / `-}}`)も効く。
/// それ以外の式(printf / range 等)は解釈しない。テンプレートエンジンを
/// 抱え込むことになるし、「どこまで動くのか」が誰にも分からなくなる。
fn vault_template(value: &str) -> Result<String, String> {
    let t = value.trim();
    let (rendered, consumed, _, _) = render_with_block(t)?;
    if !t[consumed..].trim().is_empty() {
        return Err(CANON.to_string());
    }
    Ok(rendered)
}

/// `{{ with secret "<パス>" }} … {{ end }}` を s の先頭(最初の `{{`)から1つ描画する。
/// 返り値: (描画結果, 消費バイト数, 開きタグの左trim, 閉じタグの右trim)。
fn render_with_block(s: &str) -> Result<(String, usize, bool, bool), String> {
    let Some((body, mut rest, open_left, open_right)) = take_block(s) else {
        return Err(CANON.to_string());
    };
    let Some(path) = body
        .strip_prefix("with secret")
        .map(str::trim)
        .and_then(|p| p.strip_prefix('"'))
        .and_then(|p| p.strip_suffix('"'))
        .filter(|p| !p.is_empty())
    else {
        return Err(CANON.to_string());
    };
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    let mut out = String::new();
    // 直前のタグが右trim(`-}}`)なら、続く地の文の左端の空白を削る(Go と同じ)
    let mut trim_pending = open_right;
    loop {
        let Some(brace) = rest.find("{{") else {
            return Err("vault template が {{ end }} で閉じていません".to_string());
        };
        let mut text = &rest[..brace];
        let Some((body, r, left, right)) = take_block(&rest[brace..]) else {
            return Err(CANON.to_string());
        };
        if trim_pending {
            text = text.trim_start();
        }
        if left {
            text = text.trim_end();
        }
        out.push_str(text);
        if body == "end" {
            return Ok((out, s.len() - r.len(), open_left, right));
        }
        let Some(field) = body
            .strip_prefix(".Data.data.")
            .map(str::trim)
            .filter(|f| !f.is_empty() && !f.contains(char::is_whitespace))
        else {
            return Err(CANON.to_string());
        };
        out.push_str(&vault_fetch(&segs, field)?);
        rest = r;
        trim_pending = right;
    }
}

/// 先頭の `{{ … }}` を1つ取る(手前の空白は読み飛ばす)。Go template の空白制御
/// (`{{- … -}}`。`-` はタグの内側、空白を挟んで書く)を認識して、
/// (中身, 残り, 左trim, 右trim) を返す。
fn take_block(s: &str) -> Option<(&str, &str, bool, bool)> {
    let inner = s.trim_start().strip_prefix("{{")?;
    let (inner, left) = match inner.strip_prefix('-') {
        // Go の規約どおり `-` の後には空白が要る
        Some(r) if r.starts_with(char::is_whitespace) => (r, true),
        _ => (inner, false),
    };
    let end = inner.find("}}")?;
    let rest = &inner[end + 2..];
    let body = inner[..end].trim_end();
    let (body, right) = match body.strip_suffix('-') {
        // 同じく `-` の前には空白が要る(パス末尾の `-` と紛れない)
        Some(b) if b.ends_with(char::is_whitespace) => (b, true),
        _ => (body, false),
    };
    Some((body.trim(), rest, left, right))
}

/// 明示的な受け渡し(SPEC §10.2)。サブコマンド名の**前**のグローバルフラグ。
/// フラグを打ったこと自体が同意。haj は環境を勝手に走査しない。
pub enum Delivery {
    /// `--secret <名前>=<値>` — 展開して環境変数で渡す。参照でなければ平文のまま
    Secret { name: String, value: String },
    /// `--env-file <ファイル>` — `key = value` を読み、値全体規則で展開して渡す
    EnvFile(String),
    /// `--secret-file <名前|パス>=<参照|テンプレート>`
    ///
    /// - 右辺が**参照**なら、その値がファイルの中身になる
    /// - 右辺が**それ以外**なら、テンプレートファイルのパスとみなして描画する
    ///   (参照とファイルパスは形が被らないので曖昧にならない)
    /// - 左辺が `/` を含まない**名前**なら、一時ファイルに書き、そのパスを
    ///   環境変数 `<名前>` に入れる(`KUBECONFIG` や `GOOGLE_APPLICATION_CREDENTIALS`
    ///   のように、ツールが「パスを環境変数で」要求する形にそのまま嵌まる)
    /// - 左辺が**パス**(`/` を含む)なら、そこに書く(`~/.npmrc` など固定要求向け)
    SecretFile { target: String, spec: String },
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
            "--env-file" => Ok(Delivery::EnvFile(arg.to_string())),
            "--secret-file" => split()
                .map(|(t, v)| Delivery::SecretFile {
                    target: t.to_string(),
                    spec: v.to_string(),
                })
                .ok_or_else(|| {
                    format!("--secret-file は <名前|パス>=<参照|テンプレート> で指定してください: {arg}")
                }),
            other => Err(format!("不明なフラグです: {other}")),
        }
    }

    /// 何が渡るのかを、**解決せずに**列挙する(`haj secrets --check`)。
    /// 返すのは (種別, 名前, 値または参照)。金庫には触らない。
    pub fn plan(&self) -> Result<Vec<(String, String, String)>, String> {
        match self {
            Delivery::Secret { name, value } => {
                Ok(vec![("--secret".to_string(), name.clone(), value.clone())])
            }
            Delivery::EnvFile(file) => {
                let content = std::fs::read_to_string(expand_tilde(file))
                    .map_err(|e| format!("--env-file {file}: 読めません: {e}"))?;
                let mut rows: Vec<(String, String, String)> = crate::config::parse_kv(&content)
                    .into_iter()
                    .map(|(k, v)| ("--env-file".to_string(), k, v))
                    .collect();
                rows.sort();
                Ok(rows)
            }
            Delivery::SecretFile { target, spec } => {
                let where_to = if is_path(target) {
                    target.clone()
                } else {
                    format!("(一時ファイル。パスは環境変数 {target} に入る)")
                };
                let what = if is_reference(spec) {
                    spec.clone()
                } else {
                    let content = std::fs::read_to_string(spec)
                        .map_err(|e| format!("--secret-file {spec}: 読めません: {e}"))?;
                    // テンプレート内の参照を数えるだけ。描画も解決もしない。
                    let refs = content.matches("{{").count() + content.matches("op://").count();
                    format!("{spec} (テンプレート。{refs} 個の参照を描画)")
                };
                Ok(vec![("--secret-file".to_string(), where_to, what)])
            }
        }
    }

    /// 解決して proc に適用する。書いた順に呼ぶこと(同名は後勝ち)。
    /// 失敗したら Err — 呼び出し側は本体を実行せずに止まる(fail-fast)。
    pub fn apply(&self, proc: &mut Proc) -> Result<(), String> {
        match self {
            Delivery::Secret { name, value } => {
                // 明示なので op の埋め込みも展開する
                let v = expand(value, true)
                    .map_err(|e| format!("--secret {name}: {e}"))?
                    .unwrap_or_else(|| value.clone());
                proc.env(name, v);
                Ok(())
            }
            Delivery::EnvFile(file) => {
                let content = std::fs::read_to_string(expand_tilde(file))
                    .map_err(|e| format!("--env-file {file}: 読めません: {e}"))?;
                for (k, v) in crate::config::parse_kv(&content) {
                    // 値全体規則。ファイルの値は埋め込みを解釈しない
                    let v = expand(&v, false)
                        .map_err(|e| format!("--env-file {file}: {k}: {e}"))?
                        .unwrap_or(v);
                    proc.env(k, v);
                }
                Ok(())
            }
            Delivery::SecretFile { target, spec } => {
                // 中身を作る。参照ならその値、そうでなければテンプレートを描画。
                let content = if is_reference(spec) {
                    expand(spec, true)
                        .map_err(|e| format!("--secret-file {target}: {e}"))?
                        .unwrap_or_else(|| spec.clone())
                } else {
                    let tpl = std::fs::read_to_string(expand_tilde(spec))
                        .map_err(|e| format!("--secret-file {spec}: 読めません: {e}"))?;
                    render_template(&tpl).map_err(|e| format!("--secret-file {spec}: {e}"))?
                };

                // 書き先を決める。名前なら一時ファイル + パスを環境変数へ。
                let path = if is_path(target) {
                    expand_tilde(target)
                } else {
                    runtime_dir()?.join(target)
                };
                write_secret_file(&path, &content)
                    .map_err(|e| format!("--secret-file {}: 書けません: {e}", path.display()))?;
                if !is_path(target) {
                    proc.env(target, &path);
                }
                Ok(())
            }
        }
    }
}

/// 左辺が**環境変数の名前**か。妥当な名前(英数字と `_`、先頭は数字でない)のときだけ
/// 名前とみなし、それ以外(`config.ini` / `~/.npmrc` / `./out` など)はパスとして扱う。
///
/// `/` の有無だけで見ると `config.ini` が「名前」になってしまい、環境変数として
/// 妥当でないものを export する羽目になる。
fn is_path(target: &str) -> bool {
    !is_env_name(target)
}

fn is_env_name(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with(|c: char| c.is_ascii_digit())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn expand_tilde(path: &str) -> std::path::PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = env::var_os("HOME") {
            return std::path::PathBuf::from(home).join(rest);
        }
    }
    std::path::PathBuf::from(path)
}

/// シークレットの一時ファイルを置くディレクトリ。**cwd には決して書かない**
/// (リポジトリに置かれて commit される事故を防ぐ)。
///
/// `$XDG_RUNTIME_DIR`(tmpfs。ログアウトで消える)を優先し、無ければ `$TMPDIR`。
/// mode 0700 の、この実行専用のディレクトリを作る。
///
/// **haj はこのファイルを消さない。** コアは exec(2) で自分を置き換えるため、
/// 実行後の後始末は構造的に不可能(SPEC §10.4)。
fn runtime_dir() -> Result<std::path::PathBuf, String> {
    use std::os::unix::fs::PermissionsExt;
    let base = env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(env::temp_dir);
    let dir = base.join(format!("haj.{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{} を作れません: {e}", dir.display()))?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|e| format!("{} の権限を設定できません: {e}", dir.display()))?;
    Ok(dir)
}

/// `--secret-file` のテンプレート描画。vault の正準形ブロックを置換し、
/// op:// を含めばファイル全体を `op inject` に通す(SPEC §10.2)。
/// `vault://` 短縮形はテンプレート内では解釈しない(トークンの境界が曖昧になる)。
fn render_template(content: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = content;
    while let Some(start) = rest.find("{{") {
        let (rendered, consumed, trim_left, trim_right) = render_with_block(&rest[start..])?;
        let text = &rest[..start];
        out.push_str(if trim_left { text.trim_end() } else { text });
        out.push_str(&rendered);
        rest = &rest[start + consumed..];
        if trim_right {
            rest = rest.trim_start();
        }
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
/// 実行後の後始末は構造的に不可能(SPEC §10.4)。
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
    run_cli(proc, &cli, None)
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
    run_cli(proc, &cli, Some(value))
}

/// リゾルバCLIを実行して stdout を採る。stderr はそのまま流す(失敗の理由は
/// CLI自身が一番よく知っている)。タイムアウトは設けない — op のタッチ認証など、
/// 人を待つ場面が正当にある。規約フックの2秒とは別物。
fn run_cli(mut proc: Proc, cli: &str, stdin: Option<&str>) -> Result<String, String> {
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

/// `haj secrets --check` — **何が渡るのかを、解決せずに**確かめる(SPEC §10.6)。
///
/// グローバルフラグ(`--secret` / `--env` / `--secretfile`)で渡そうとしているものを
/// 列挙する。参照の**対象**(パス)は出すが、**値は解決しない** — 金庫に問い合わせない
/// ので、OIDC ログインもタッチ認証も起きない(参照そのものは秘密ではない)。
///
///   haj --secret DB_PASS=vault://... --env-file ./mig.env secrets --check
pub fn run(args: &[String], deliveries: &[Delivery]) -> ! {
    if args.first().map(String::as_str) != Some("--check") {
        eprintln!(
            "haj: 使い方: haj [--secret <名前>=<値>]... [--env-file <ファイル>]... \
             [--secret-file <名前|パス>=<参照|テンプレート>]... secrets --check"
        );
        std::process::exit(1);
    }

    if deliveries.is_empty() {
        println!("渡すシークレットの指定がありません。");
        println!("  haj --secret DB_PASS=vault://... --env-file ./mig.env secrets --check");
        std::process::exit(0);
    }

    println!(" 実行時に渡るもの (値は解決していません):");
    for d in deliveries {
        match d.plan() {
            Ok(rows) => {
                for (kind, name, value) in rows {
                    let mark = if is_reference(&value) { "→" } else { " " };
                    println!("   {kind:10}  {name:20}  {mark} {value}");
                }
            }
            Err(e) => {
                eprintln!("haj: {e}");
                std::process::exit(1);
            }
        }
    }
    println!("\n (→ が付いたものが展開されます。他は平文としてそのまま渡ります)");
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
