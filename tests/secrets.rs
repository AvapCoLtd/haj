//! シークレット参照の展開(SPEC.md §10)を外から確かめる。
//!
//! 金庫には触らない。偽の vault / op を置き、HAJ_VAULT_CMD / HAJ_OP_CMD で
//! 差し替えて全経路を通す。

use std::fs;

/// ビルドプロファイル(src/profile.rs と同じ判定)。テストは同じ環境で
/// コンパイルされるので、本体と必ず一致する。
const AVAP_PROFILE: bool = option_env!("HAJ_BUILD_AVAP").is_some();
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};

/// テストごとに独立した作業ディレクトリ(cli.rs と同じ流儀。tempfile は使わない)。
struct Sandbox {
    dir: PathBuf,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("haj-secrets-test-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    /// ただのファイルを置く(テンプレートや env ファイル用)。
    fn write_file(&self, rel: &str, body: &str) {
        let path = self.dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    /// 実行可能ファイルを置く。
    fn exe(&self, rel: &str, body: &str) -> PathBuf {
        let path = self.dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// 環境変数 HAJ_T_VALUE をそのまま出すサブコマンドを sys ツリーに置く。
    fn show_command(&self) -> String {
        self.exe(
            "sys/commands/show",
            "#!/bin/sh\ncase \"$1\" in --haj-*) exit 0 ;; esac\nprintf '%s\\n' \"$HAJ_T_VALUE\"\n",
        );
        self.dir.join("sys/commands").to_string_lossy().to_string()
    }

    /// 実行の痕跡(marker)を残すサブコマンド。fail-fast の検証用。
    fn mark_command(&self) -> String {
        let marker = self.dir.join("ran");
        self.exe(
            "sys/commands/mark",
            &format!(
                "#!/bin/sh\ncase \"$1\" in --haj-*) exit 0 ;; esac\ntouch \"{}\"\n",
                marker.display()
            ),
        );
        self.dir.join("sys/commands").to_string_lossy().to_string()
    }

    /// 受け取った引数を1行ずつ出すサブコマンド。argv 展開の検証用。
    fn args_command(&self) -> String {
        self.exe(
            "sys/commands/args",
            "#!/bin/sh\ncase \"$1\" in --haj-*) exit 0 ;; esac\nprintf '%s\\n' \"$@\"\n",
        );
        self.dir.join("sys/commands").to_string_lossy().to_string()
    }

    /// 偽の vault。受けた引数を記録して固定の値を返す。
    fn fake_vault(&self) -> PathBuf {
        let record = self.dir.join("vault-args");
        self.exe(
            "bin/vault",
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\nprintf 's3cr3t\\n'\n",
                record.display()
            ),
        )
    }

    /// 偽の op。inject を要求し、stdin の op:// を置換して返す。
    fn fake_op(&self) -> PathBuf {
        self.exe(
            "bin/op",
            "#!/bin/sh\n[ \"$1\" = inject ] || exit 9\nsed 's|op://[^ ]*|RESOLVED|g'\n",
        )
    }

    /// haj を走らせる。展開系の環境変数は毎回明示する(親環境を漏らさない)。
    fn haj(&self, command_path: &str, args: &[&str], envs: &[(&str, &str)]) -> Output {
        let mut c = Command::new(env!("CARGO_BIN_EXE_haj"));
        c.args(args)
            .current_dir(&self.dir)
            .env("HAJ_COMMAND_PATH", command_path)
            .env("HAJ_NO_CACHE", "1")
            .env("HOME", &self.dir)
            .env("XDG_CONFIG_HOME", self.dir.join(".config"))
            .env_remove("HAJ_SECRETS")
            .env_remove("HAJ_OP_CMD")
            .env_remove("HAJ_VAULT_CMD")
            .env_remove("HAJ_VAULT_LOGIN")
            .env_remove("VAULT_ADDR")
            .env_remove("BAO_ADDR")
            .env_remove("HAJ_T_VALUE");
        for (k, v) in envs {
            c.env(k, v);
        }
        c.output().unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).to_string()
}

#[test]
fn env参照は別の環境変数の値になる() {
    let sb = Sandbox::new("env");
    let cp = sb.show_command();

    let out = sb.haj(
        &cp,
        &[
            "--secret",
            &format!("HAJ_T_VALUE={}", "env://HAJ_T_SRC"),
            "show",
        ],
        &[("HAJ_T_SRC", "hello")],
    );
    assert!(out.status.success());
    assert_eq!(stdout(&out).trim(), "hello");
}

#[test]
fn file参照はファイルの中身になり末尾の改行を落とす() {
    let sb = Sandbox::new("file");
    let cp = sb.show_command();
    let f = sb.dir.join("cred");
    fs::write(
        &f,
        "t0ps3cret
",
    )
    .unwrap();

    let out = sb.haj(
        &cp,
        &[
            "--secret",
            &format!("HAJ_T_VALUE=file://{}", f.display()),
            "show",
        ],
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out),
        "t0ps3cret
"
    ); // 中身の改行は show が付けた1つだけ
}

#[test]
fn vault_uri形は最後のセグメントがフィールド() {
    let sb = Sandbox::new("vault-uri");
    let cp = sb.show_command();
    let vault = sb.fake_vault();

    let out = sb.haj(
        &cp,
        &[
            "--secret",
            &format!("HAJ_T_VALUE={}", "vault://secret/data/app/db_password"),
            "show",
        ],
        &[("HAJ_VAULT_CMD", vault.to_str().unwrap())],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "s3cr3t");

    // /data/ 入りパス(template の書き方)は mount と相対パスに読み替える
    let args = fs::read_to_string(sb.dir.join("vault-args")).unwrap();
    assert_eq!(args, "kv\nget\n-field=db_password\n-mount=secret\napp\n");
}

#[test]
fn vault_template正準形はuri形と同じ解決になる() {
    let sb = Sandbox::new("vault-tpl");
    let cp = sb.show_command();
    let vault = sb.fake_vault();

    let out = sb.haj(
        &cp,
        &[
            "--secret",
            r#"HAJ_T_VALUE={{ with secret "secret/data/app" }}{{ .Data.data.db_password }}{{ end }}"#,
            "show",
        ],
        &[("HAJ_VAULT_CMD", vault.to_str().unwrap())],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "s3cr3t");

    let args = fs::read_to_string(sb.dir.join("vault-args")).unwrap();
    assert_eq!(
        args,
        "kv
get
-field=db_password
-mount=secret
app
"
    );
}

#[test]
fn vault_templateの正準形以外は中止する() {
    let sb = Sandbox::new("vault-canon");
    let cp = sb.mark_command();

    let out = sb.haj(
        &cp,
        &["--secret", r#"HAJ_T_VALUE={{ printf "x" }}"#, "mark"],
        &[],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("解釈できません"),
        "stderr: {}",
        stderr(&out)
    );
    assert!(!sb.dir.join("ran").exists(), "本体が実行されてしまった");
}

#[test]
fn 解決に失敗したら本体を実行しない() {
    let sb = Sandbox::new("failfast");
    let cp = sb.mark_command();
    let vault = sb.exe("bin/vault", "#!/bin/sh\nexit 1\n");

    let out = sb.haj(
        &cp,
        &[
            "--secret",
            &format!("HAJ_T_VALUE={}", "vault://secret/data/app/db_password"),
            "mark",
        ],
        &[("HAJ_VAULT_CMD", vault.to_str().unwrap())],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(!sb.dir.join("ran").exists(), "本体が実行されてしまった");
}

#[test]
fn リゾルバの実行ファイルは設定でも差し替えられる() {
    let sb = Sandbox::new("config");
    let cp = sb.show_command();
    let vault = sb.fake_vault();

    // HAJ_VAULT_CMD を使わず、~/.config/haj/config の vault_cmd で差し替える
    let confdir = sb.dir.join(".config/haj");
    fs::create_dir_all(&confdir).unwrap();
    fs::write(
        confdir.join("config"),
        format!("secrets.vault_cmd = {}\n", vault.display()),
    )
    .unwrap();

    let out = sb.haj(
        &cp,
        &[
            "--secret",
            &format!("HAJ_T_VALUE={}", "vault://secret/data/app/db_password"),
            "show",
        ],
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "s3cr3t");
}

#[test]
fn secretsのcheckは渡るものを解決せずに列挙する() {
    let sb = Sandbox::new("dryrun");
    let cp = sb.show_command();
    sb.write_file(
        "mig.env",
        "DB_HOST = db.internal
DB_USER = vault://secret/data/db/user
",
    );

    // 偽 vault すら置かない。解決しに行けば失敗するが、dry-run は金庫に触らない。
    let out = sb.haj(
        &cp,
        &[
            "--secret",
            "DB_PASS=vault://secret/data/db/password",
            "--env-file",
            "mig.env",
            "secrets",
            "--check",
        ],
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(
        s.contains("DB_PASS"),
        "--secret が出ていない:
{s}"
    );
    assert!(
        s.contains("vault://secret/data/db/password"),
        "参照の対象が出ていない:
{s}"
    );
    assert!(
        s.contains("DB_USER"),
        "--env の参照が出ていない:
{s}"
    );
    assert!(
        s.contains("db.internal"),
        "--env の平文が出ていない:
{s}"
    );

    // 引数なしは使い方エラー
    assert_eq!(sb.haj(&cp, &["secrets"], &[]).status.code(), Some(1));
}

#[test]
fn 規約フックには展開しない() {
    let sb = Sandbox::new("hook");
    // --haj-describe が HAJ_T_VALUE をそのまま説明文として返すコマンド。
    // フックの経路で展開されるなら、ここに展開後の値が現れてしまう。
    sb.exe(
        "sys/commands/leaky",
        "#!/bin/sh
case \"$1\" in --haj-describe) printf '%s\n' \"$HAJ_T_VALUE\"; exit 0 ;; --haj-*) exit 0 ;; esac
",
    );
    let cp = sb.dir.join("sys/commands");

    // 親環境に参照が入っていても、フック(commands の一覧)では展開されない
    let out = sb.haj(
        cp.to_str().unwrap(),
        &["commands"],
        &[
            ("HAJ_T_VALUE", "env://HAJ_T_SRC"),
            ("HAJ_T_SRC", "expanded"),
        ],
    );
    assert!(out.status.success());
    let s = stdout(&out);
    assert!(
        !s.contains("expanded"),
        "フックに展開が漏れている:
{s}"
    );
}

/// 状態を持つ偽 vault。`token lookup` はログイン済みのときだけ 0、
/// `login` は引数を記録してログイン済みにし、`kv get` はログイン済みのときだけ
/// 答える(そのとき見えている VAULT_ADDR も記録する)。
fn stateful_vault(sb: &Sandbox) -> std::path::PathBuf {
    let state = sb.dir.join("vault-state");
    let login_args = sb.dir.join("login-args");
    let addr = sb.dir.join("seen-addr");
    sb.exe(
        "bin/vault-login",
        &format!(
            "#!/bin/sh\ncase \"$1\" in\n  token) [ -f \"{state}\" ] && exit 0 || exit 2 ;;\n  login) shift; printf '%s ' \"$@\" > \"{login}\"; touch \"{state}\"; exit 0 ;;\n  kv) [ -f \"{state}\" ] || exit 2; printf '%s\\n' \"$VAULT_ADDR\" > \"{addr}\"; printf 's3cr3t\\n' ;;\nesac\n",
            state = state.display(),
            login = login_args.display(),
            addr = addr.display()
        ),
    )
}

const LOGIN_ARGS: &str = "-method=oidc -path=<パス>";

#[test]
fn 未ログインなら既定の引数で自動ログインしてから解決する() {
    let sb = Sandbox::new("autologin");
    let cp = sb.show_command();
    let vault = stateful_vault(&sb);

    // vault_login は何も設定しない → 既定で動きが変わる(ビルドプロファイル)
    let out = sb.haj(
        &cp,
        &[
            "--secret",
            &format!("HAJ_T_VALUE={}", "vault://secret/data/app/db_password"),
            "show",
        ],
        &[("HAJ_VAULT_CMD", vault.to_str().unwrap())],
    );

    if AVAP_PROFILE {
        // vault_login を設定した場合
        assert!(out.status.success(), "stderr: {}", stderr(&out));
        assert_eq!(stdout(&out).trim(), "s3cr3t");

        let args = fs::read_to_string(sb.dir.join("login-args")).unwrap();
        assert_eq!(args.trim(), "-method=oidc");

        // サーバの既定も CLI に渡っている
        let addr = fs::read_to_string(sb.dir.join("seen-addr")).unwrap();
        assert_eq!(addr.trim(), "https://vault.example.com");
    } else {
        // 公開プロファイル: 既定は off。勝手にログインせず、解決の失敗で止まる
        assert_eq!(out.status.code(), Some(1));
        assert!(
            !sb.dir.join("login-args").exists(),
            "公開既定でloginが走った"
        );
    }
}

#[test]
fn vault_loginの設定が既定の引数を上書きする() {
    let sb = Sandbox::new("loginargs");
    let cp = sb.show_command();
    let vault = stateful_vault(&sb);

    let out = sb.haj(
        &cp,
        &[
            "--secret",
            &format!("HAJ_T_VALUE={}", "vault://secret/data/app/db_password"),
            "show",
        ],
        &[
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("HAJ_VAULT_LOGIN", LOGIN_ARGS),
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "s3cr3t");

    let args = fs::read_to_string(sb.dir.join("login-args")).unwrap();
    assert_eq!(args.trim(), LOGIN_ARGS);
}

#[test]
fn vault_login_offなら自動ログインせず解決の失敗で止まる() {
    let sb = Sandbox::new("nologin");
    let cp = sb.mark_command();
    let vault = stateful_vault(&sb);

    let out = sb.haj(
        &cp,
        &[
            "--secret",
            &format!("HAJ_T_VALUE={}", "vault://secret/data/app/db_password"),
            "mark",
        ],
        &[
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("HAJ_VAULT_LOGIN", "off"),
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(!sb.dir.join("login-args").exists(), "loginが勝手に走った");
    assert!(!sb.dir.join("ran").exists(), "本体が実行されてしまった");
}

#[test]
fn ログイン済みならloginを叩かない() {
    let sb = Sandbox::new("loggedin");
    let cp = sb.show_command();
    let vault = stateful_vault(&sb);
    fs::write(sb.dir.join("vault-state"), "").unwrap(); // ログイン済みにしておく

    let out = sb.haj(
        &cp,
        &[
            "--secret",
            &format!("HAJ_T_VALUE={}", "vault://secret/data/app/db_password"),
            "show",
        ],
        &[
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("HAJ_VAULT_LOGIN", LOGIN_ARGS),
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "s3cr3t");
    assert!(
        !sb.dir.join("login-args").exists(),
        "ログイン済みなのにloginが走った"
    );
}

// ---- 明示的な受け渡し(SPEC §10.2): --secret / --env-file / --secret-file ----

#[test]
fn secretフラグは展開して環境変数で渡す() {
    let sb = Sandbox::new("flag-secret");
    let cp = sb.show_command();
    let vault = sb.fake_vault();

    // HAJ_SECRETS は立てない。フラグを打ったこと自体が同意
    let out = sb.haj(
        &cp,
        &[
            "--secret",
            "HAJ_T_VALUE=vault://secret/data/app/db_password",
            "show",
        ],
        &[("HAJ_VAULT_CMD", vault.to_str().unwrap())],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "s3cr3t");
}

#[test]
fn secretフラグの参照でない値は平文としてそのまま渡る() {
    let sb = Sandbox::new("flag-plain");
    let cp = sb.show_command();

    let out = sb.haj(&cp, &["--secret", "HAJ_T_VALUE=hello", "show"], &[]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "hello");
}

#[test]
fn 名前の後のフラグは解釈されずそのまま渡る() {
    let sb = Sandbox::new("flag-after");
    let cp = sb.args_command();

    // SPEC §11: 名前以降は無解釈で素通し
    let out = sb.haj(&cp, &["args", "--secret", "X=y"], &[]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "--secret\nX=y\n");
}

#[test]
fn envフラグはファイルの値を値全体規則で展開して渡す() {
    let sb = Sandbox::new("flag-env");
    let cp = sb.show_command();
    let vault = sb.fake_vault();
    sb.write_file(
        "mig.env",
        "HAJ_T_VALUE = vault://secret/data/app/db_password\nHAJ_T_NOTE = 文中の op://x はただの文字列\n",
    );

    let out = sb.haj(
        &cp,
        &["--env-file", "mig.env", "show"],
        &[("HAJ_VAULT_CMD", vault.to_str().unwrap())],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "s3cr3t");
}

#[test]
fn secretフラグはenvフラグより後に書けば勝つ() {
    let sb = Sandbox::new("flag-order");
    let cp = sb.show_command();
    sb.write_file("a.env", "HAJ_T_VALUE = ファイルの値\n");

    let out = sb.haj(
        &cp,
        &[
            "--env-file",
            "a.env",
            "--secret",
            "HAJ_T_VALUE=あとの値",
            "show",
        ],
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "あとの値");
}

#[test]
fn secret_fileはテンプレートを描画して0600で書く() {
    let sb = Sandbox::new("flag-file");
    let cp = sb.mark_command();
    let vault = sb.fake_vault();
    let op = sb.fake_op();
    sb.write_file(
        "config.ini.tpl",
        "[db]\npassword = {{ with secret \"secret/data/app\" }}{{ .Data.data.db_password }}{{ end }}\ntoken = op://Infra/ci/token\n",
    );

    let out = sb.haj(
        &cp,
        &["--secret-file", "config.ini=config.ini.tpl", "mark"],
        &[
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("HAJ_OP_CMD", op.to_str().unwrap()),
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(sb.dir.join("ran").exists(), "本体が実行されていない");

    let rendered = fs::read_to_string(sb.dir.join("config.ini")).unwrap();
    assert_eq!(rendered, "[db]\npassword = s3cr3t\ntoken = RESOLVED\n");

    let mode = fs::metadata(sb.dir.join("config.ini"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "mode が 0600 ではない: {mode:o}");
}

#[test]
fn vault_agentの実テンプレートを描画できる() {
    // 空白制御 ({{- -}})、ブロック内の地の文、複数フィールド。
    // vault-agent で使っているテンプレートがそのまま動くこと(Issue #3 の約束)。
    let sb = Sandbox::new("flag-file-agent");
    let cp = sb.mark_command();
    let vault = sb.fake_vault();
    sb.write_file(
        "config.yml.tpl",
        "hosts:\n  example.test:\n    token: {{ with secret \"users/me/gitlab\" -}}\n      {{ .Data.data.token }}\n    {{- end }}\n    user: {{ with secret \"users/me/gitlab\" }}u={{ .Data.data.user }} t={{ .Data.data.token }}{{ end }}\n",
    );

    let out = sb.haj(
        &cp,
        &["--secret-file", "config.yml=config.yml.tpl", "mark"],
        &[("HAJ_VAULT_CMD", vault.to_str().unwrap())],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let rendered = fs::read_to_string(sb.dir.join("config.yml")).unwrap();
    assert_eq!(
        rendered,
        "hosts:\n  example.test:\n    token: s3cr3t\n    user: u=s3cr3t t=s3cr3t\n"
    );
}

#[test]
fn secret_fileのテンプレートパスと環境ファイルのチルダは展開される() {
    let sb = Sandbox::new("flag-tilde");
    let cp = sb.mark_command();
    let vault = sb.fake_vault();
    // HOME = sb.dir なので ~/t.tpl はサンドボックス内
    sb.write_file(
        "t.tpl",
        "x = {{ with secret \"secret/data/app\" }}{{ .Data.data.f }}{{ end }}\n",
    );
    sb.write_file("vars.env", "TILDE_OK = yes\n");

    let out = sb.haj(
        &cp,
        &[
            "--secret-file",
            "out.ini=~/t.tpl",
            "--env-file",
            "~/vars.env",
            "mark",
        ],
        &[("HAJ_VAULT_CMD", vault.to_str().unwrap())],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        fs::read_to_string(sb.dir.join("out.ini")).unwrap(),
        "x = s3cr3t\n"
    );
}

#[test]
fn secret_fileの解決に失敗したら書かずに止まる() {
    let sb = Sandbox::new("flag-file-fail");
    let cp = sb.mark_command();
    let vault = sb.exe("bin/vault", "#!/bin/sh\nexit 1\n");
    sb.write_file(
        "bad.tpl",
        "x = {{ with secret \"secret/data/app\" }}{{ .Data.data.db_password }}{{ end }}\n",
    );

    let out = sb.haj(
        &cp,
        &["--secret-file", "out.ini=bad.tpl", "mark"],
        &[
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("HAJ_VAULT_LOGIN", "off"),
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(!sb.dir.join("out.ini").exists(), "半端なファイルが残った");
    assert!(!sb.dir.join("ran").exists(), "本体が実行されてしまった");
}

#[test]
fn フラグの後にコマンドが無ければ使い方エラー() {
    let sb = Sandbox::new("flag-usage");
    let cp = sb.show_command();

    let out = sb.haj(&cp, &["--secret", "K=v"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("使い方"), "stderr: {}", stderr(&out));

    // 組み込みコマンドに続けて書くのも誤り
    let out = sb.haj(&cp, &["--secret", "K=v", "help"], &[]);
    assert_eq!(out.status.code(), Some(1));
}

// ---- haj exec(SPEC §9.2): 探索を通さず PATH のコマンドに注入して実行 ----

#[test]
fn execはpathのコマンドに注入して実行する() {
    let sb = Sandbox::new("exec");
    let cp = sb.show_command(); // HAJ_COMMAND_PATH には dbtool は無い
    let vault = sb.fake_vault();
    sb.exe(
        "extbin/dbtool",
        "#!/bin/sh\nprintf '%s\\n' \"$HAJ_T_VALUE\"\n",
    );
    let path = format!(
        "{}:{}",
        sb.dir.join("extbin").display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = sb.haj(
        &cp,
        &[
            "--secret",
            "HAJ_T_VALUE=vault://secret/data/app/db_password",
            "exec",
            "dbtool",
        ],
        &[("HAJ_VAULT_CMD", vault.to_str().unwrap()), ("PATH", &path)],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "s3cr3t");
}

#[test]
fn execはシェルを明示すれば変数展開が使える() {
    let sb = Sandbox::new("exec-sh");
    let cp = sb.show_command();

    let out = sb.haj(
        &cp,
        &[
            "--secret",
            "HAJ_T_VALUE=hello",
            "exec",
            "sh",
            "-c",
            "printf '%s' \"$HAJ_T_VALUE\"",
        ],
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "hello");
}

#[test]
fn execは探索のコマンドを見ずhaj環境変数も渡さない() {
    let sb = Sandbox::new("exec-isolated");
    // 探索ツリーに同名 dbtool を置くが、PATH には置かない → exec からは見えない
    let cp = sb.show_command();
    sb.exe("sys/commands/dbtool", "#!/bin/sh\necho from-tree\n");

    let out = sb.haj(&cp, &["exec", "dbtool"], &[]);
    assert_eq!(out.status.code(), Some(127), "stderr: {}", stderr(&out));

    // HAJ_ROOT / HAJ_PROJECT は exec には渡らない(親環境に残っていても消す)
    let out = sb.haj(
        &cp,
        &[
            "exec",
            "sh",
            "-c",
            "printf '%s' \"${HAJ_ROOT:-none}:${HAJ_PROJECT:-none}\"",
        ],
        &[("HAJ_ROOT", "/stale"), ("HAJ_PROJECT", "stale")],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "none:none");
}

#[test]
fn execは予約語なので探索コマンドに奪われない() {
    let sb = Sandbox::new("exec-reserved");
    let cp = sb.show_command();
    // 悪意ある exec を探索ツリーに置いても、組み込みが常に勝つ
    let marker = sb.dir.join("stolen");
    sb.exe(
        "sys/commands/exec",
        &format!("#!/bin/sh\ntouch \"{}\"\n", marker.display()),
    );

    let out = sb.haj(&cp, &["exec", "sh", "-c", "true"], &[]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(!marker.exists(), "探索の exec に奪われた");
}

#[test]
fn execの引数なしは使い方エラー() {
    let sb = Sandbox::new("exec-usage");
    let cp = sb.show_command();

    let out = sb.haj(&cp, &["exec"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("使い方"), "stderr: {}", stderr(&out));
}

// ---- haj sh(SPEC §9.2): exec sh -c の省略形 ----

#[test]
fn shはシェルの1行に注入して実行する() {
    let sb = Sandbox::new("sh");
    let cp = sb.show_command();
    let vault = sb.fake_vault();

    let out = sb.haj(
        &cp,
        &[
            "--secret",
            "HAJ_T_VALUE=vault://secret/data/app/db_password",
            "sh",
            "printf '%s' \"$HAJ_T_VALUE\"",
        ],
        &[("HAJ_VAULT_CMD", vault.to_str().unwrap())],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "s3cr3t");
}

#[test]
fn shの追加引数は位置パラメータになる() {
    let sb = Sandbox::new("sh-args");
    let cp = sb.show_command();

    let out = sb.haj(&cp, &["sh", "printf '%s' \"$1-$2\"", "one", "two"], &[]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "one-two");
}

#[test]
fn shの引数なしは使い方エラー() {
    let sb = Sandbox::new("sh-usage");
    let cp = sb.show_command();

    let out = sb.haj(&cp, &["sh"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("使い方"), "stderr: {}", stderr(&out));
}
// ---- 設定の token に参照を書ける(SPEC §8.4) ----

#[test]
fn 設定のtokenの参照はselfupgradeが使うときに展開される() {
    let sb = Sandbox::new("token-ref");
    let cp = sb.show_command();
    let vault = sb.fake_vault();

    // 偽 curl: 受けた引数を記録し、最新リリースとして現行版を返す
    sb.exe(
        "extbin/curl",
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"{}\"\nprintf '{{\"tag_name\":\"v{}\"}}'\n",
            sb.dir.join("curl-args").display(),
            env!("CARGO_PKG_VERSION")
        ),
    );
    let path = format!(
        "{}:{}",
        sb.dir.join("extbin").display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let confdir = sb.dir.join(".config/haj");
    fs::create_dir_all(&confdir).unwrap();
    fs::write(
        confdir.join("config"),
        "selfupgrade.token = vault://secret/data/haj/token\n\
         selfupgrade.gitlab = https://gitlab.example.test\n\
         selfupgrade.project_id = 1\n",
    )
    .unwrap();

    let out = sb.haj(
        &cp,
        &["selfupgrade", "--check"],
        &[
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("PATH", &path),
            ("HAJ_TOKEN", ""), // 空は未設定扱い → 設定ファイルの参照が効く
        ],
    );
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out)); // 最新
    let args = fs::read_to_string(sb.dir.join("curl-args")).unwrap();
    assert!(
        args.contains("PRIVATE-TOKEN: s3cr3t"),
        "展開されたトークンで認証していない:\n{args}"
    );
}

#[test]
fn configはtokenの参照をそのまま出す() {
    let sb = Sandbox::new("token-show");
    let cp = sb.show_command();

    let confdir = sb.dir.join(".config/haj");
    fs::create_dir_all(&confdir).unwrap();
    fs::write(
        confdir.join("config"),
        "selfupgrade.token = vault://secret/data/haj/token\n",
    )
    .unwrap();

    let out = sb.haj(&cp, &["config"], &[("HAJ_TOKEN", "")]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(
        s.contains("vault://secret/data/haj/token"),
        "参照が表示されていない:\n{s}"
    );
    assert!(!s.contains("********"), "参照なのにマスクされた:\n{s}");
}

#[test]
fn 平文のtokenは従来どおりマスクされる() {
    let sb = Sandbox::new("token-mask");
    let cp = sb.show_command();

    let out = sb.haj(&cp, &["config"], &[("HAJ_TOKEN", "glpat-plainvalue")]);
    assert!(out.status.success());
    let s = stdout(&out);
    assert!(s.contains("********"), "平文がマスクされていない:\n{s}");
    assert!(!s.contains("glpat-plainvalue"), "平文が漏れた:\n{s}");
}

// ---- haj config --init(SPEC §8.2): 設定の雛形 ----

#[test]
fn config_initは全ての鍵と既定値を雛形として出す() {
    let sb = Sandbox::new("config-init");
    let cp = sb.show_command();

    let out = sb.haj(&cp, &["config", "--init"], &[]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);

    // 全ての鍵が出ている
    for key in [
        "command_path",
        "hook_timeout_ms",
        "secrets.op_cmd",
        "secrets.vault_cmd",
        "secrets.vault_addr",
        "secrets.vault_login",
        "selfupgrade.gitlab",
        "selfupgrade.project_id",
        "selfupgrade.target",
        "selfupgrade.token",
    ] {
        assert!(
            s.contains(&format!("# {key} ")) || s.contains(&format!("# {key} =")),
            "{key} が雛形に無い:\n{s}"
        );
    }
    // 既定値も出ている(ビルドプロファイルで変わる)
    if AVAP_PROFILE {
        assert!(
            s.contains("https://vault.example.com"),
            "既定値が無い:\n{s}"
        );
        assert!(s.contains("-method=oidc"), "既定値が無い:\n{s}");
    } else {
        assert!(
            s.contains("secrets.vault_login = off"),
            "公開既定が off でない:\n{s}"
        );
        assert!(
            !s.contains("https://vault.example.com"),
            "公開ビルドに社内の既定値が焼き込まれている:\n{s}"
        );
    }

    // 全行コメントか空行 = そのまま置いても挙動が変わらない
    for line in s.lines() {
        assert!(
            line.is_empty() || line.starts_with('#'),
            "コメントでない行がある: {line}"
        );
    }
}

// ---- exec / sh の `--`(SPEC §9.2): 指癖との互換 ----

#[test]
fn execは先頭のダッシュダッシュを読み飛ばす() {
    let sb = Sandbox::new("exec-dd");
    let cp = sb.show_command();
    sb.exe("extbin/dbtool", "#!/bin/sh\nprintf 'dd-ok\\n'\n");
    let path = format!(
        "{}:{}",
        sb.dir.join("extbin").display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let out = sb.haj(&cp, &["exec", "--", "dbtool"], &[("PATH", &path)]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "dd-ok");
}

#[test]
fn shのダッシュダッシュは語を繋いで1行にする() {
    let sb = Sandbox::new("sh-dd");
    let cp = sb.show_command();

    // haj sh -- ls -la の形。語が空白で繋がれて1行のスクリプトになる
    let out = sb.haj(&cp, &["sh", "--", "printf", "'%s'", "joined-ok"], &[]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "joined-ok");
}

#[test]
fn shはスクリプトがダッシュで始まっても誤解釈しない() {
    let sb = Sandbox::new("sh-dash");
    let cp = sb.show_command();

    // 以前は sh が「-」をオプションと解釈し、$0 用の "haj" をコマンドとして
    // 実行してしまっていた(haj sh -- ls -la が haj のヘルプを出す珍事の原因)。
    let out = sb.haj(&cp, &["sh", "-not-an-option"], &[]);
    assert_eq!(out.status.code(), Some(127)); // シェルが「コマンドが無い」と言う
    let s = stderr(&out);
    assert!(
        !s.contains("使い方: haj <コマンド>"),
        "hajが再帰的に走った: {s}"
    );
}

#[test]
fn selfupgradeはgithub形式のjsonも読める() {
    let sb = Sandbox::new("gh-json");
    let cp = sb.show_command();

    // GitHub API は "tag_name": "v1.2.3"(コロンの後に空白)。
    // GitLab は "tag_name":"v1.2.3"。決め打ちすると片方で読めなくなる。
    sb.exe(
        "extbin/curl",
        &format!(
            "#!/bin/sh\nprintf '{{\\n  \"tag_name\": \"v{}\",\\n  \"name\": \"x\"\\n}}'\n",
            env!("CARGO_PKG_VERSION")
        ),
    );
    let path = format!(
        "{}:{}",
        sb.dir.join("extbin").display(),
        std::env::var("PATH").unwrap_or_default()
    );

    // 既定の取得元(GitHub)。認証なしで最新版を読み、現行版と同じ = 0
    let out = sb.haj(&cp, &["selfupgrade", "--check"], &[("PATH", &path)]);
    assert_eq!(out.status.code(), Some(0), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("最新です"),
        "stdout: {}",
        stdout(&out)
    );
}

#[test]
fn secret_fileは参照の値をファイルに書きパスを環境変数に入れる() {
    let sb = Sandbox::new("sf-ref");
    let vault = sb.fake_vault();
    // $KEY にパスが入り、その中身が値になっていることを確認するコマンド
    sb.exe(
        "sys/commands/readkey",
        "#!/bin/sh\ncase \"$1\" in --haj-*) exit 0 ;; esac\nprintf 'path=%s content=%s\\n' \"$KEY\" \"$(cat \"$KEY\")\"\n",
    );
    let cp = sb.dir.join("sys/commands");
    let runtime = sb.dir.join("run");
    fs::create_dir_all(&runtime).unwrap();

    let out = sb.haj(
        cp.to_str().unwrap(),
        &[
            "--secret-file",
            "KEY=vault://secret/data/ssh/id_rsa",
            "readkey",
        ],
        &[
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("XDG_RUNTIME_DIR", runtime.to_str().unwrap()),
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(
        s.contains("content=s3cr3t"),
        "値がファイルに入っていない:\n{s}"
    );
    assert!(
        s.contains(&format!("path={}", runtime.display())),
        "一時ファイルが XDG_RUNTIME_DIR に作られていない:\n{s}"
    );
    // cwd には書かれない(リポジトリ汚染の防止)
    assert!(!sb.dir.join("KEY").exists(), "cwd に書かれた");
}

#[test]
fn secret_fileの名前スラッシュ形はディレクトリを環境変数に入れる() {
    // GLAB_CONFIG_DIR のように「設定ディレクトリを環境変数で指せ」と要求する
    // ツール向け(SPEC §10.4)。
    let sb = Sandbox::new("flag-file-envdir");
    let cp = sb.show_command();
    let vault = sb.fake_vault();
    sb.write_file(
        "c.tpl",
        "token: {{ with secret \"users/me/gitlab\" }}{{ .Data.data.token }}{{ end }}\n",
    );

    // sh がディレクトリ変数を展開して中のファイルを読めること = 実際の使われ方
    let out = sb.haj(
        &cp,
        &[
            "--secret-file",
            "GLAB_CONFIG_DIR/config.yml=c.tpl",
            "sh",
            "--",
            "cat",
            "$GLAB_CONFIG_DIR/config.yml",
        ],
        &[
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("XDG_RUNTIME_DIR", sb.dir.to_str().unwrap()),
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "token: s3cr3t\n");

    // 変数の中身はディレクトリで、名前で終わる
    let out = sb.haj(
        &cp,
        &[
            "--secret-file",
            "GLAB_CONFIG_DIR/config.yml=c.tpl",
            "sh",
            "--",
            "printf",
            "%s",
            "$GLAB_CONFIG_DIR",
        ],
        &[
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("XDG_RUNTIME_DIR", sb.dir.to_str().unwrap()),
        ],
    );
    let dir = stdout(&out);
    assert!(
        dir.ends_with("/GLAB_CONFIG_DIR"),
        "ディレクトリでない: {dir}"
    );

    // 小文字が混ざる先頭セグメントは相対パスのまま(奪わない)
    fs::create_dir_all(sb.dir.join("outdir")).unwrap();
    let out2 = sb.haj(
        &cp,
        &[
            "--secret-file",
            "outdir/config.yml=c.tpl",
            "sh",
            "--",
            "cat",
            "outdir/config.yml",
        ],
        &[("HAJ_VAULT_CMD", vault.to_str().unwrap())],
    );
    assert!(out2.status.success(), "stderr: {}", stderr(&out2));
    assert_eq!(stdout(&out2), "token: s3cr3t\n");
}

#[test]
fn secret_fileはパス指定ならそこに0600で書く() {
    let sb = Sandbox::new("sf-path");
    let vault = sb.fake_vault();
    let cp = sb.mark_command();
    let target = sb.dir.join("creds/id_rsa");

    let out = sb.haj(
        &cp,
        &[
            "--secret-file",
            &format!("{}=vault://secret/data/ssh/id_rsa", target.display()),
            "mark",
        ],
        &[("HAJ_VAULT_CMD", vault.to_str().unwrap())],
    );
    // 親ディレクトリが無いので失敗する(半端なファイルを残さない)
    assert_eq!(out.status.code(), Some(1));

    // 親を作れば書ける
    fs::create_dir_all(sb.dir.join("creds")).unwrap();
    let out = sb.haj(
        &cp,
        &[
            "--secret-file",
            &format!("{}=vault://secret/data/ssh/id_rsa", target.display()),
            "mark",
        ],
        &[("HAJ_VAULT_CMD", vault.to_str().unwrap())],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(fs::read_to_string(&target).unwrap(), "s3cr3t");
    let mode = fs::metadata(&target).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "mode が 0600 ではない: {mode:o}");
}

// ---- ツリー専用ストア(SPEC §10.7–10.9)とツリーごとの設定注入(§10.8) ----

/// インストール済みツリー tools のコマンド show を置く(HAJ_T_VALUE を出す)。
fn tree_show(sb: &Sandbox) {
    sb.exe(
        ".local/share/haj/trees/tools/commands/show",
        "#!/bin/sh\ncase \"$1\" in\n  --haj-env) printf 'HAJ_T_VALUE=%s\\n' \"${HAJ_T_VALUE:-}\"; exit 0 ;;\n  --haj-*) exit 0 ;;\nesac\nprintf '%s\\n' \"$HAJ_T_VALUE\"\n",
    );
}

/// put の経路を試す偽 vault。オブジェクト/フィールドの有無はファイルで制御し、
/// patch / put は stdin を kv-written に写す。
fn fake_kv(sb: &Sandbox) -> PathBuf {
    let d = sb.dir.display().to_string();
    sb.exe(
        "bin/vault",
        &format!(
            r#"#!/bin/sh
d="{d}"
printf '%s\n' "$*" >> "$d/vault-calls"
case "$1" in
  kv)
    case "$2" in
      get)
        case "$*" in
          *-field=*) if [ -e "$d/kv-field" ]; then echo oldvalue; exit 0; else exit 2; fi ;;
          *) if [ -e "$d/kv-object" ]; then exit 0; else exit 2; fi ;;
        esac ;;
      patch|put) echo "$2" > "$d/kv-verb"; cat > "$d/kv-written"; exit 0 ;;
    esac ;;
esac
exit 0
"#
        ),
    )
}

fn read(sb: &Sandbox, rel: &str) -> String {
    fs::read_to_string(sb.dir.join(rel)).unwrap_or_default()
}

#[test]
fn store参照は自ツリーの名前空間へ写像されて解決される() {
    let sb = Sandbox::new("store-map");
    let vault = sb.fake_vault();
    tree_show(&sb);
    let cp = sb.dir.join("nonexistent").display().to_string();

    // tree.secret 注入で store:// を使う(文脈はツリー tools)
    sb.write_file(
        ".config/haj/config",
        "tree.tools.secret.HAJ_T_VALUE = store://google-oauth/token\n",
    );
    let out = sb.haj(
        &cp,
        &["tools", "show"],
        &[
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("USER", "alice"),
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "s3cr3t");
    let args = read(&sb, "vault-args");
    assert!(
        args.contains("-mount=secret")
            && args.contains("-field=token")
            && args.contains("users/alice/trees/tools/google-oauth"),
        "物理写像が違う:\n{args}"
    );
}

#[test]
fn store参照はツリー文脈が無ければ実行前に止まる() {
    let sb = Sandbox::new("store-noctx");
    let cp = sb.mark_command();

    let out = sb.haj(
        &cp,
        &["--secret", "HAJ_T_VALUE=store://token", "mark"],
        &[("USER", "alice")],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("ツリーのコマンドの中でだけ"),
        "案内が無い: {}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("vault://"),
        "物理参照での点検の案内が無い: {}",
        stderr(&out)
    );
    assert!(!sb.dir.join("ran").exists(), "fail-fast していない");
}

#[test]
fn storeゲットは環境のhaj_treeで解決して値と改行を出す() {
    let sb = Sandbox::new("store-get");
    let vault = sb.fake_vault();
    let cp = sb.dir.join("nonexistent").display().to_string();

    let out = sb.haj(
        &cp,
        &["store", "get", "store://token"],
        &[
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("HAJ_TREE", "tools"),
            ("USER", "alice"),
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "s3cr3t\n");
    let args = read(&sb, "vault-args");
    assert!(
        args.contains("users/alice/trees/tools") && args.contains("-field=token"),
        "写像が違う:\n{args}"
    );

    // 文脈なしはエラー
    let out = sb.haj(
        &cp,
        &["store", "get", "store://token"],
        &[
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("USER", "alice"),
        ],
    );
    assert_eq!(out.status.code(), Some(1));
}

#[test]
fn storeプットは新規はputで既存オブジェクトはpatchで書く() {
    let sb = Sandbox::new("store-put");
    let vault = fake_kv(&sb);
    let _cp = sb.dir.join("nonexistent").display().to_string();
    let envs: Vec<(&str, String)> = vec![
        ("HAJ_VAULT_CMD", vault.display().to_string()),
        ("HAJ_TREE", "tools".to_string()),
        ("USER", "alice".to_string()),
    ];
    let envs: Vec<(&str, &str)> = envs.iter().map(|(k, v)| (*k, v.as_str())).collect();

    // オブジェクトが無い → kv put
    let mut c = Command::new(env!("CARGO_BIN_EXE_haj"));
    c.args(["store", "put", "store://token"])
        .current_dir(&sb.dir)
        .env("HAJ_COMMAND_PATH", "nonexistent")
        .env("HAJ_NO_CACHE", "1")
        .env("HOME", &sb.dir)
        .env("XDG_CONFIG_HOME", sb.dir.join(".config"))
        .env_remove("VAULT_ADDR")
        .env_remove("BAO_ADDR");
    for (k, v) in &envs {
        c.env(k, v);
    }
    use std::process::Stdio;
    c.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    use std::io::Write as _;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"newtoken\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(read(&sb, "kv-verb").trim(), "put", "新規は kv put のはず");
    assert_eq!(
        read(&sb, "kv-written"),
        "newtoken",
        "末尾の改行が落ちていない"
    );

    // オブジェクトはあるがフィールドは無い → kv patch
    fs::write(sb.dir.join("kv-object"), "").unwrap();
    let mut child = c.spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"second").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        read(&sb, "kv-verb").trim(),
        "patch",
        "既存オブジェクトは patch のはず"
    );

    // フィールドもある → --force 無しでは拒否(書かない)
    fs::write(sb.dir.join("kv-field"), "").unwrap();
    let _ = fs::remove_file(sb.dir.join("kv-written"));
    let mut child = c.spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"third").unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("--force"),
        "上書きの案内が無い: {}",
        stderr(&out)
    );
    assert!(
        !sb.dir.join("kv-written").exists(),
        "拒否したのに書いている"
    );

    // --force なら patch で上書き
    let mut c2 = Command::new(env!("CARGO_BIN_EXE_haj"));
    c2.args(["store", "put", "--force", "store://token"])
        .current_dir(&sb.dir)
        .env("HAJ_COMMAND_PATH", "nonexistent")
        .env("HAJ_NO_CACHE", "1")
        .env("HOME", &sb.dir)
        .env("XDG_CONFIG_HOME", sb.dir.join(".config"))
        .env_remove("VAULT_ADDR")
        .env_remove("BAO_ADDR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in &envs {
        c2.env(k, v);
    }
    let mut child = c2.spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"forced").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(read(&sb, "kv-verb").trim(), "patch");
    assert_eq!(read(&sb, "kv-written"), "forced");
}

#[test]
fn tree設定のenvは無展開で注入されsecretは解決して注入される() {
    let sb = Sandbox::new("tree-inject");
    let vault = sb.fake_vault();
    tree_show(&sb);
    let cp = sb.dir.join("nonexistent").display().to_string();

    // .env は store:// と書いてあっても文字列のまま(参照をデータとして渡せる)
    sb.write_file(
        ".config/haj/config",
        "tree.tools.env.HAJ_T_VALUE = store://token\n",
    );
    let out = sb.haj(&cp, &["show"], &[("USER", "alice")]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(
        stdout(&out).trim(),
        "store://token",
        ".env が展開されている"
    );

    // .secret は解決して注入(素の探索でも名前空間形でも)
    sb.write_file(
        ".config/haj/config",
        "tree.tools.secret.HAJ_T_VALUE = vault://secret/data/x/token\n",
    );
    let out = sb.haj(
        &cp,
        &["show"],
        &[
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("USER", "alice"),
        ],
    );
    assert_eq!(stdout(&out).trim(), "s3cr3t", ".secret が解決されない");
}

#[test]
fn tree設定よりシェル環境とフラグが勝つ() {
    let sb = Sandbox::new("tree-precedence");
    tree_show(&sb);
    let cp = sb.dir.join("nonexistent").display().to_string();
    sb.write_file(
        ".config/haj/config",
        "tree.tools.env.HAJ_T_VALUE = config-value\n",
    );

    // シェル環境 > tree設定
    let out = sb.haj(&cp, &["show"], &[("HAJ_T_VALUE", "shell-value")]);
    assert_eq!(stdout(&out).trim(), "shell-value");

    // フラグ > シェル環境 > tree設定
    let out = sb.haj(
        &cp,
        &["--secret", "HAJ_T_VALUE=flag-value", "show"],
        &[("HAJ_T_VALUE", "shell-value")],
    );
    assert_eq!(stdout(&out).trim(), "flag-value");
}

#[test]
fn tree設定のsecretに平文を書くと実行前に止まる() {
    let sb = Sandbox::new("tree-plain-secret");
    tree_show(&sb);
    let cp = sb.dir.join("nonexistent").display().to_string();
    sb.write_file(
        ".config/haj/config",
        "tree.tools.secret.HAJ_T_VALUE = plainpassword\n",
    );
    let out = sb.haj(&cp, &["show"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("参照ではありません") && stderr(&out).contains("tree.tools.env"),
        "案内が無い: {}",
        stderr(&out)
    );
}

#[test]
fn ツリー自身のconfigのtree鍵は注入されない() {
    let sb = Sandbox::new("tree-hijack");
    tree_show(&sb);
    // ツリー自身の config に tree.* を書いても効かない(盗み先の指定になるため)
    sb.write_file(
        ".local/share/haj/trees/tools/config",
        "tree.tools.env.HAJ_T_VALUE = evil\n",
    );
    let cp = sb.dir.join("nonexistent").display().to_string();
    let out = sb.haj(&cp, &["show"], &[]);
    assert!(out.status.success());
    assert_eq!(
        stdout(&out).trim(),
        "",
        "ツリー config の tree.* が効いている"
    );
}

#[test]
fn hajエンブはtree設定の出所を注記しsecretは参照のまま出す() {
    let sb = Sandbox::new("env-annotate");
    tree_show(&sb);
    let cp = sb.dir.join("nonexistent").display().to_string();
    sb.write_file(
        ".config/haj/config",
        "tree.tools.env.HAJ_T_VALUE = fixed\ntree.tools.secret.HAJ_T_SECRET = vault://secret/data/x/token\n",
    );
    // show の --haj-env は HAJ_T_VALUE しか申告しないので、HAJ_T_SECRET も申告する版を置く
    sb.exe(
        ".local/share/haj/trees/tools/commands/show",
        "#!/bin/sh\ncase \"$1\" in\n  --haj-env) printf 'HAJ_T_VALUE=%s\\nHAJ_T_SECRET=%s\\n' \"${HAJ_T_VALUE:-}\" \"${HAJ_T_SECRET:-}\"; exit 0 ;;\n  --haj-*) exit 0 ;;\nesac\ntrue\n",
    );

    let out = sb.haj(&cp, &["env", "show"], &[]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(
        s.contains("HAJ_T_VALUE=fixed") && s.contains("# tree設定 (env)"),
        "env の注記が無い:\n{s}"
    );
    assert!(
        s.contains("HAJ_T_SECRET=vault://secret/data/x/token") && s.contains("# tree設定 (secret"),
        "secret が参照のまま注記されていない:\n{s}"
    );

    // シェル環境が勝っている鍵の注記
    let out = sb.haj(&cp, &["env", "show"], &[("HAJ_T_VALUE", "shell")]);
    let s = stdout(&out);
    assert!(
        s.contains("HAJ_T_VALUE=shell") && s.contains("# シェル環境"),
        "シェル環境の注記が無い:\n{s}"
    );
}

#[test]
fn secretsチェックはstore参照に物理写像を添える() {
    let sb = Sandbox::new("store-check");
    let cp = sb.dir.join("nonexistent").display().to_string();

    // ツリー文脈(HAJ_TREE)があれば具体的な写像
    let out = sb.haj(
        &cp,
        &["--secret", "T=store://token", "secrets", "--check"],
        &[("HAJ_TREE", "tools"), ("USER", "alice")],
    );
    assert!(out.status.success());
    let s = stdout(&out);
    assert!(
        s.contains("store://token")
            && s.contains("vault://secret/data/users/alice/trees/tools/token"),
        "写像が出ない:\n{s}"
    );

    // 文脈が無ければ形だけ(エラーにしない)
    let out = sb.haj(
        &cp,
        &["--secret", "T=store://token", "secrets", "--check"],
        &[("USER", "alice")],
    );
    assert!(out.status.success(), "文脈なしの --check が失敗する");
    assert!(
        stdout(&out).contains("<HAJ_TREE>"),
        "写像の形が出ない:\n{}",
        stdout(&out)
    );
}

#[test]
fn 規約フックにはtreeのenvだけ注入されsecretは解決されない() {
    let sb = Sandbox::new("hook-inject");
    // describe が HAJ_T_VALUE と HAJ_T_SECRET を出す
    sb.exe(
        ".local/share/haj/trees/tools/commands/show",
        "#!/bin/sh\ncase \"$1\" in\n  --haj-describe) printf 'v=%s s=%s\\n' \"${HAJ_T_VALUE:-none}\" \"${HAJ_T_SECRET:-none}\"; exit 0 ;;\n  --haj-*) exit 0 ;;\nesac\ntrue\n",
    );
    let cp = sb.dir.join("nonexistent").display().to_string();
    sb.write_file(
        ".config/haj/config",
        "tree.tools.env.HAJ_T_VALUE = injected\ntree.tools.secret.HAJ_T_SECRET = vault://secret/data/x/y\n",
    );
    // 偽 vault を置かない — フックが .secret を解決しようとすれば失敗で気づく
    let out = sb.haj(&cp, &["tools"], &[]);
    let s = stdout(&out);
    assert!(
        s.contains("v=injected s=none"),
        "フックへの注入が仕様と違う (env だけのはず):\n{s}"
    );
}

#[test]
fn store設定は表形式のキーで効き旧キーは警告して無視される() {
    let sb = Sandbox::new("store-table");
    let vault = sb.fake_vault();
    let cp = sb.dir.join("nonexistent").display().to_string();

    // 新キー store.tree.prefix が写像に効く
    sb.write_file(".config/haj/config", "store.tree.prefix = kv/data/team\n");
    let out = sb.haj(
        &cp,
        &["store", "get", "store://token"],
        &[
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("HAJ_TREE", "tools"),
            ("USER", "alice"),
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let args = fs::read_to_string(sb.dir.join("vault-args")).unwrap_or_default();
    assert!(
        args.contains("-mount=kv") && args.contains("team/trees/tools"),
        "store.tree.prefix が効いていない:\n{args}"
    );

    // 旧キー (0.31.0 形) は無視され、警告が出る。写像は既定に戻る
    sb.write_file(
        ".config/haj/config",
        "store.prefix = kv/data/old\nstore.engine = vault\n",
    );
    let out = sb.haj(
        &cp,
        &["store", "get", "store://token"],
        &[
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("HAJ_TREE", "tools"),
            ("USER", "alice"),
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let args = fs::read_to_string(sb.dir.join("vault-args")).unwrap_or_default();
    assert!(
        args.contains("users/alice/trees/tools") && !args.contains("old"),
        "旧キーが効いてしまっている:\n{args}"
    );
    assert!(
        stderr(&out).contains("store.tree.prefix") && stderr(&out).contains("改名"),
        "旧キーの警告が出ない: {}",
        stderr(&out)
    );

    // 環境変数の写像も新しい名前
    let out = sb.haj(
        &cp,
        &["store", "get", "store://token"],
        &[
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("HAJ_TREE", "tools"),
            ("USER", "alice"),
            ("HAJ_STORE_TREE_PREFIX", "kv/data/env"),
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let args = fs::read_to_string(sb.dir.join("vault-args")).unwrap_or_default();
    assert!(
        args.contains("env/trees/tools"),
        "HAJ_STORE_TREE_PREFIX が効いていない:\n{args}"
    );
}
