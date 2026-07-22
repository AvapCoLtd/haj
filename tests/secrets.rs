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
fn secretのcheckは渡るものを解決せずに列挙する() {
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
            "secret",
            "check",
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
fn secretゲットは宣言のstore参照を自ツリーの名前空間で解決する() {
    let sb = Sandbox::new("secret-get");
    let vault = sb.fake_vault();
    let cp = sb.dir.join("nonexistent").display().to_string();

    // 宣言(capability)。exec 時には何も起きず、get で引いたときだけ解決される
    sb.write_file(
        ".config/haj/config",
        "tree.tools.secret.HAJ_T_VALUE = store://google-oauth/token\n",
    );
    let out = sb.haj(
        &cp,
        &["secret", "get", "HAJ_T_VALUE"],
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
        args.contains("-mount=secret")
            && args.contains("-field=token")
            && args.contains("users/alice/trees/tools/google-oauth"),
        "物理写像が違う:\n{args}"
    );

    // 宣言に無い KEY はエラー(宣言済みを列挙して案内)
    let out = sb.haj(
        &cp,
        &["secret", "get", "NOPE"],
        &[("HAJ_TREE", "tools"), ("USER", "alice")],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("宣言されていません") && stderr(&out).contains("HAJ_T_VALUE"),
        "宣言済みの列挙が無い: {}",
        stderr(&out)
    );

    // ツリーの外では user 域が目録になる(相補)— tree の宣言には届かない
    let out = sb.haj(&cp, &["secret", "get", "HAJ_T_VALUE"], &[("USER", "alice")]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("user.secret"),
        "user 域の案内が無い: {}",
        stderr(&out)
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
        stderr(&out).contains("HAJ_TREE=<インストール名>"),
        "人手の HAJ_TREE 明示の案内が無い: {}",
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
    c.args(["store", "put", "token"])
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
    c2.args(["store", "put", "--force", "token"])
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
fn tree設定のenvは無展開で注入されsecretは注入されない() {
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

    // .secret は宣言 — exec 時に解決も注入もされない(秘密は env に勝手に載らない)
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
    assert!(out.status.success());
    assert_eq!(
        stdout(&out).trim(),
        "",
        ".secret が注入されている(宣言のはず)"
    );
    assert!(
        !sb.dir.join("vault-args").exists(),
        "exec 時に金庫へ触っている"
    );

    // 引くのはコマンド自身(pull): haj secret get
    let out = sb.haj(
        &cp,
        &["secret", "get", "HAJ_T_VALUE"],
        &[
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("HAJ_TREE", "tools"),
            ("USER", "alice"),
        ],
    );
    assert_eq!(
        stdout(&out),
        "s3cr3t\n",
        "get で解決されない: {}",
        stderr(&out)
    );
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
fn 平文の宣言はgetとcheckがエラーにするがexecは止めない() {
    let sb = Sandbox::new("tree-plain-secret");
    tree_show(&sb);
    let cp = sb.dir.join("nonexistent").display().to_string();
    sb.write_file(
        ".config/haj/config",
        "tree.tools.secret.HAJ_T_VALUE = plainpassword\n",
    );

    // 宣言は exec に関与しない(注入しないので、平文でも本体は動く)
    let out = sb.haj(&cp, &["show"], &[]);
    assert!(
        out.status.success(),
        "宣言が exec を止めている: {}",
        stderr(&out)
    );

    // get は平文を拒否する
    let out = sb.haj(
        &cp,
        &["secret", "get", "HAJ_T_VALUE"],
        &[("HAJ_TREE", "tools")],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("参照ではありません") && stderr(&out).contains("tree.tools.env"),
        "案内が無い: {}",
        stderr(&out)
    );

    // check もエラーとして見せる(exit 1)
    let out = sb.haj(&cp, &["secret", "check"], &[("HAJ_TREE", "tools")]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stdout(&out).contains("参照ではありません"),
        "check に平文が出ない:\n{}",
        stdout(&out)
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
fn hajエンブはtreeのenvの出所だけ注記する() {
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
    // 宣言(secret)は env とは別物 — haj env は注記しない(目録は haj secret list)
    assert!(
        !s.contains("# tree設定 (secret"),
        "secret が haj env に注記されている(宣言のはず):\n{s}"
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
        &["--secret", "T=store://token", "secret", "check"],
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
        &["--secret", "T=store://token", "secret", "check"],
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

#[test]
fn secretリストは宣言の目録を参照のまま出す() {
    let sb = Sandbox::new("secret-list");
    let cp = sb.dir.join("nonexistent").display().to_string();
    sb.write_file(
        ".config/haj/config",
        "tree.tools.secret.B_KEY = vault://secret/data/x/b\ntree.tools.secret.A_KEY = store://token\n",
    );
    let out = sb.haj(&cp, &["secret", "list"], &[("HAJ_TREE", "tools")]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(
        s.contains("A_KEY=store://token") && s.contains("B_KEY=vault://secret/data/x/b"),
        "目録が出ない:\n{s}"
    );
    assert!(
        !sb.dir.join("vault-args").exists(),
        "list が金庫に触っている"
    );

    // ツリーの外の list は user 域を見る(この設定には無いので案内)
    let out = sb.haj(&cp, &["secret", "list"], &[]);
    assert!(out.status.success());
    assert!(
        stdout(&out).contains("user.secret"),
        "user 域を見ていない:\n{}",
        stdout(&out)
    );
}

#[test]
fn secretチェックは宣言の検証も出す() {
    let sb = Sandbox::new("secret-check-decl");
    let cp = sb.dir.join("nonexistent").display().to_string();
    sb.write_file(
        ".config/haj/config",
        "tree.tools.secret.MM_TOKEN = store://token\n",
    );
    let out = sb.haj(
        &cp,
        &["secret", "check"],
        &[("HAJ_TREE", "tools"), ("USER", "alice")],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(
        s.contains("宣言 (tree.tools.secret.*)")
            && s.contains("MM_TOKEN")
            && s.contains("vault://secret/data/users/alice/trees/tools/token"),
        "宣言の検証が出ない:\n{s}"
    );
}

#[test]
fn 旧名secretsは移行スタブとして案内する() {
    let sb = Sandbox::new("secrets-stub");
    let cp = sb.dir.join("nonexistent").display().to_string();
    let out = sb.haj(
        &cp,
        &["--secret", "X=env://HOME", "secrets", "--check"],
        &[],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("haj secret check"),
        "改名の案内が無い: {}",
        stderr(&out)
    );
}

#[test]
fn secretゲットの補完は宣言済みキーを出す() {
    let sb = Sandbox::new("secret-complete");
    let cp = sb.dir.join("nonexistent").display().to_string();
    sb.write_file(
        ".config/haj/config",
        "tree.tools.secret.CLIENT_SECRET = vault://secret/data/x/c\n",
    );
    // 動詞
    let out = sb.haj(&cp, &["__complete", "secret"], &[]);
    let s = stdout(&out);
    assert!(
        s.contains("get") && s.contains("list") && s.contains("check"),
        "動詞が補完されない:\n{s}"
    );
    // get の後は宣言済み KEY(文脈があるとき)
    let out = sb.haj(
        &cp,
        &["__complete", "secret", "get"],
        &[("HAJ_TREE", "tools")],
    );
    assert!(
        stdout(&out).contains("CLIENT_SECRET"),
        "宣言済み KEY が補完されない:\n{}",
        stdout(&out)
    );
    // 文脈なしなら候補なし(エラーも出さない)
    let out = sb.haj(&cp, &["__complete", "secret", "get"], &[]);
    assert!(out.status.success());
    assert_eq!(stdout(&out).trim(), "");
}

#[test]
fn secretのlistとcheckはtreeフラグで対象を明示できる() {
    let sb = Sandbox::new("secret-tree-flag");
    let cp = sb.dir.join("nonexistent").display().to_string();
    sb.write_file(
        ".config/haj/config",
        "tree.work.secret.A_KEY = store://token\ntree.home.secret.B_KEY = vault://secret/data/x/b\n",
    );

    // --tree で対象を明示(HAJ_TREE なしで動く — 人手の口)
    let out = sb.haj(&cp, &["secret", "list", "--tree", "work"], &[]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("A_KEY=store://token"),
        "work の目録が出ない:\n{}",
        stdout(&out)
    );

    // --tree > 環境の HAJ_TREE(その場の明示が勝つ)
    let out = sb.haj(
        &cp,
        &["secret", "list", "--tree", "home"],
        &[("HAJ_TREE", "work")],
    );
    assert!(
        stdout(&out).contains("B_KEY") && !stdout(&out).contains("A_KEY"),
        "--tree が環境に勝っていない:\n{}",
        stdout(&out)
    );

    // check も同様
    let out = sb.haj(
        &cp,
        &["secret", "check", "--tree", "work"],
        &[("USER", "alice")],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("tree.work.secret") && stdout(&out).contains("trees/work/token"),
        "check --tree が効かない:\n{}",
        stdout(&out)
    );

    // ツリーの外の list は user 域を見る(相補 — tree の宣言は出ない)
    let out = sb.haj(&cp, &["secret", "list"], &[]);
    assert!(out.status.success());
    let s = stdout(&out);
    assert!(
        s.contains("user.secret") && !s.contains("A_KEY") && !s.contains("B_KEY"),
        "user 域になっていない:\n{s}"
    );
}

#[test]
fn secretのgetにはtreeフラグが無い() {
    let sb = Sandbox::new("secret-get-no-flag");
    let cp = sb.dir.join("nonexistent").display().to_string();
    sb.write_file(
        ".config/haj/config",
        "tree.work.secret.A_KEY = vault://secret/data/x/a\n",
    );
    // capability の壁: 値に触る get の対象は文脈だけで決まる
    let out = sb.haj(&cp, &["secret", "get", "--tree", "work", "A_KEY"], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("--tree はありません") && stderr(&out).contains("HAJ_TREE"),
        "get の --tree 拒否と案内が無い: {}",
        stderr(&out)
    );
}

#[test]
fn configのtreeフラグはインスタンスの設定と名前空間を出す() {
    let sb = Sandbox::new("config-tree");
    let cp = sb.dir.join("nonexistent").display().to_string();
    sb.write_file(
        ".config/haj/config",
        "tree.work.env.MYAPP_ACCOUNT = alice@example.com\ntree.work.secret.CLIENT_SECRET = vault://secret/data/x/c\n",
    );

    let out = sb.haj(&cp, &["config", "--tree", "work"], &[("USER", "alice")]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(
        s.contains("tree.work.env.MYAPP_ACCOUNT")
            && s.contains("alice@example.com")
            && s.contains("(設定ファイル)"),
        "env の実効値と出所が出ない:\n{s}"
    );
    assert!(
        s.contains("tree.work.secret.CLIENT_SECRET")
            && s.contains("vault://secret/data/x/c")
            && s.contains("宣言"),
        "宣言が参照のまま出ない:\n{s}"
    );
    assert!(
        s.contains("trees/work/") && s.contains("store の名前空間"),
        "store の名前空間が出ない:\n{s}"
    );
    assert!(
        !sb.dir.join("vault-args").exists(),
        "config --tree が金庫に触っている"
    );

    // シェル環境が勝っている鍵の表示
    let out = sb.haj(
        &cp,
        &["config", "--tree", "work"],
        &[("USER", "alice"), ("MYAPP_ACCOUNT", "shell@example.com")],
    );
    let s = stdout(&out);
    assert!(
        s.contains("shell@example.com") && s.contains("シェル環境が優先"),
        "シェル環境の優先が出ない:\n{s}"
    );

    // 設定が無いツリーは案内
    let out = sb.haj(&cp, &["config", "--tree", "nothing"], &[("USER", "alice")]);
    assert!(out.status.success());
    assert!(
        stdout(&out).contains("設定はありません"),
        "空の案内が無い:\n{}",
        stdout(&out)
    );
}

// ---- 0.38.0: haj secret file と user.secret.*(SPEC §10.8 / §10.9) ----

#[test]
fn user域の宣言はツリーの外でだけ引ける() {
    let sb = Sandbox::new("user-secret");
    let vault = sb.fake_vault();
    let cp = sb.dir.join("nonexistent").display().to_string();
    sb.write_file(
        ".config/haj/config",
        "user.secret.OCI_KEY = vault://secret/data/oci/private_key\n",
    );

    // ツリーの外(HAJ_TREE なし)で解決できる
    let out = sb.haj(
        &cp,
        &["secret", "get", "OCI_KEY"],
        &[
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("USER", "alice"),
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "s3cr3t\n");

    // 相補: ツリー文脈からは user.* に届かない
    let out = sb.haj(
        &cp,
        &["secret", "get", "OCI_KEY"],
        &[
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("HAJ_TREE", "tools"),
            ("USER", "alice"),
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("宣言されていません") && stderr(&out).contains("tree.tools.secret"),
        "ツリー文脈から user.* に届いている: {}",
        stderr(&out)
    );

    // list もツリーの外では user 域
    let out = sb.haj(&cp, &["secret", "list"], &[]);
    assert!(out.status.success());
    assert!(
        stdout(&out).contains("OCI_KEY=vault://secret/data/oci/private_key"),
        "user 域の目録が出ない:\n{}",
        stdout(&out)
    );

    // check もツリーの外では user 域を検証する
    let out = sb.haj(&cp, &["secret", "check"], &[("USER", "alice")]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stdout(&out).contains("宣言 (user.secret.*)") && stdout(&out).contains("OCI_KEY"),
        "user 域の検証が出ない:\n{}",
        stdout(&out)
    );
}

#[test]
fn user域の宣言にstore参照は書けない() {
    let sb = Sandbox::new("user-secret-store");
    let cp = sb.dir.join("nonexistent").display().to_string();
    sb.write_file(".config/haj/config", "user.secret.TOKEN = store://token\n");

    // get は明示的に拒否(ユーザーに store の名前空間は無い)
    let out = sb.haj(&cp, &["secret", "get", "TOKEN"], &[("USER", "alice")]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("user 域では使えません"),
        "store:// の拒否が無い: {}",
        stderr(&out)
    );

    // check もエラーとして見せる
    let out = sb.haj(&cp, &["secret", "check"], &[("USER", "alice")]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stdout(&out).contains("store:// は user 域では使えません"),
        "check に出ない:\n{}",
        stdout(&out)
    );
}

#[test]
fn secretファイルは実体化してパスを出し呼ぶたび上書きする() {
    let sb = Sandbox::new("secret-file");
    let vault = sb.fake_vault();
    let cp = sb.dir.join("nonexistent").display().to_string();
    let runtime = sb.dir.join("runtime");
    fs::create_dir_all(&runtime).unwrap();
    sb.write_file(
        ".config/haj/config",
        "user.secret.OCI_KEY = vault://secret/data/oci/private_key\n",
    );

    let out = sb.haj(
        &cp,
        &["secret", "file", "OCI_KEY"],
        &[
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("USER", "alice"),
            ("XDG_RUNTIME_DIR", runtime.to_str().unwrap()),
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let path = stdout(&out).trim().to_string();
    assert!(
        path.ends_with("haj/secret-files/OCI_KEY"),
        "パスの形が違う: {path}"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "s3cr3t", "中身が違う");
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "モードが 0600 でない: {mode:o}");
    let dir_mode = fs::metadata(std::path::Path::new(&path).parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700, "ディレクトリが 0700 でない: {dir_mode:o}");

    // 呼ぶたび上書き(同じパスに落ち着く)
    let out2 = sb.haj(
        &cp,
        &["secret", "file", "OCI_KEY"],
        &[
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("USER", "alice"),
            ("XDG_RUNTIME_DIR", runtime.to_str().unwrap()),
        ],
    );
    assert_eq!(stdout(&out2).trim(), path, "同じ KEY で別のパスになった");

    // XDG_RUNTIME_DIR が無い環境はフォールバック(uid 付き tmp)
    let out3 = sb.haj(
        &cp,
        &["secret", "file", "OCI_KEY"],
        &[
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("USER", "alice"),
        ],
    );
    assert!(out3.status.success(), "stderr: {}", stderr(&out3));
    assert!(
        stdout(&out3).contains("haj-") && stdout(&out3).contains("secret-files/OCI_KEY"),
        "フォールバックのパスの形が違う: {}",
        stdout(&out3)
    );
    let _ = fs::remove_file(stdout(&out3).trim());
}

#[test]
fn secretファイルも宣言解決の規則はgetと同一() {
    let sb = Sandbox::new("secret-file-rules");
    let cp = sb.dir.join("nonexistent").display().to_string();
    sb.write_file(
        ".config/haj/config",
        "user.secret.OCI_KEY = vault://secret/data/oci/private_key\n",
    );

    // 宣言に無い KEY はエラー(capability)
    let out = sb.haj(&cp, &["secret", "file", "NOPE"], &[("USER", "alice")]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("宣言されていません"));

    // --tree は拒否(get と同じ壁)
    let out = sb.haj(
        &cp,
        &["secret", "file", "--tree", "tools", "OCI_KEY"],
        &[("USER", "alice")],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("--tree はありません"),
        "file の --tree 拒否が無い: {}",
        stderr(&out)
    );

    // 補完の動詞に file が出る
    let out = sb.haj(&cp, &["__complete", "secret"], &[]);
    assert!(stdout(&out).contains("file"), "動詞 file が補完されない");
    // file の後は宣言済み KEY(ツリーの外なら user 域)
    let out = sb.haj(&cp, &["__complete", "secret", "file"], &[]);
    assert!(
        stdout(&out).contains("OCI_KEY"),
        "user 域の KEY が補完されない:\n{}",
        stdout(&out)
    );
}

// ---- 0.39.0: テンプレート宣言 (template / tmpdir。SPEC §10.8 / §10.9) ----

#[test]
fn tmpdirは同じ名前で常に同じパスの0700ディレクトリを返す() {
    let sb = Sandbox::new("tmpdir");
    let cp = sb.dir.join("nonexistent").display().to_string();
    let runtime = sb.dir.join("runtime");
    fs::create_dir_all(&runtime).unwrap();
    let envs = [("XDG_RUNTIME_DIR", runtime.to_str().unwrap())];

    let out = sb.haj(&cp, &["secret", "tmpdir", "glab"], &envs);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let path = stdout(&out).trim().to_string();
    assert!(path.ends_with("haj/tmpdir/glab"), "パスの形が違う: {path}");
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o700, "0700 でない: {mode:o}");

    // 同じ名前は常に同じパス
    let out2 = sb.haj(&cp, &["secret", "tmpdir", "glab"], &envs);
    assert_eq!(stdout(&out2).trim(), path);

    // 名前の字面: パス区切りや .. は構造的に不可
    for bad in ["../evil", "a/b", ".hidden", "-x", ""] {
        let out = sb.haj(&cp, &["secret", "tmpdir", bad], &envs);
        assert_eq!(out.status.code(), Some(1), "{bad} が通ってしまった");
    }
}

#[test]
fn テンプレート宣言は描画して実体化しパスを出す() {
    let sb = Sandbox::new("tpl-render");
    let vault = sb.fake_vault();
    let cp = sb.dir.join("nonexistent").display().to_string();
    let runtime = sb.dir.join("runtime");
    fs::create_dir_all(&runtime).unwrap();
    sb.write_file(
        "glab.yml.tpl",
        "host: example.com\ntoken: {{ with secret \"secret/data/glab\" }}{{ .Data.data.token }}{{ end }}\n",
    );
    sb.write_file(
        ".config/haj/config",
        &format!(
            "user.template.GLAB_CONFIG = {}\n",
            sb.dir.join("glab.yml.tpl").display()
        ),
    );
    let envs = [
        ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
        ("XDG_RUNTIME_DIR", runtime.to_str().unwrap()),
        ("USER", "alice"),
    ];

    // 既定の書き先 (管理領域の templates/<KEY>)
    let out = sb.haj(&cp, &["secret", "template", "GLAB_CONFIG"], &envs);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let path = stdout(&out).trim().to_string();
    assert!(
        path.ends_with("haj/templates/GLAB_CONFIG"),
        "パスの形が違う: {path}"
    );
    let body = fs::read_to_string(&path).unwrap();
    assert_eq!(
        body, "host: example.com\ntoken: s3cr3t\n",
        "描画が違う: {body}"
    );
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "0600 でない: {mode:o}");

    // --out: tmpdir の中なら書ける(使い姿の合成)
    let dir = stdout(&sb.haj(&cp, &["secret", "tmpdir", "glab"], &envs))
        .trim()
        .to_string();
    let out_file = format!("{dir}/config.yml");
    let out = sb.haj(
        &cp,
        &["secret", "template", "GLAB_CONFIG", "--out", &out_file],
        &envs,
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), out_file);
    assert!(fs::read_to_string(&out_file).unwrap().contains("s3cr3t"));

    // 宣言に無い KEY はエラー
    let out = sb.haj(&cp, &["secret", "template", "NOPE"], &envs);
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("テンプレート宣言にありません"));
}

#[test]
fn テンプレートのoutは管理領域の外に書けない() {
    let sb = Sandbox::new("tpl-out-escape");
    let vault = sb.fake_vault();
    let cp = sb.dir.join("nonexistent").display().to_string();
    let runtime = sb.dir.join("runtime");
    fs::create_dir_all(&runtime).unwrap();
    sb.write_file("t.tpl", "plain\n");
    sb.write_file(
        ".config/haj/config",
        &format!("user.template.T = {}\n", sb.dir.join("t.tpl").display()),
    );
    let envs = [
        ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
        ("XDG_RUNTIME_DIR", runtime.to_str().unwrap()),
        ("USER", "alice"),
    ];

    // 管理領域の外は拒否(秘密の実体が任意の永続パスに書かれる口は無い)
    let outside = sb.dir.join("leak.yml");
    let out = sb.haj(
        &cp,
        &[
            "secret",
            "template",
            "T",
            "--out",
            outside.to_str().unwrap(),
        ],
        &envs,
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("管理領域") && stderr(&out).contains("外には書けません"),
        "領域外拒否の案内が無い: {}",
        stderr(&out)
    );
    assert!(!outside.exists(), "領域外に書かれている");

    // シンボリックリンクで外へ抜ける道も塞ぐ(realpath 検証)
    let dir = stdout(&sb.haj(&cp, &["secret", "tmpdir", "t"], &envs))
        .trim()
        .to_string();
    let escape = sb.dir.join("escape-target");
    fs::create_dir_all(&escape).unwrap();
    std::os::unix::fs::symlink(&escape, format!("{dir}/link")).unwrap();
    let out = sb.haj(
        &cp,
        &[
            "secret",
            "template",
            "T",
            "--out",
            &format!("{dir}/link/x.yml"),
        ],
        &envs,
    );
    assert_eq!(out.status.code(), Some(1), "symlink 脱出が通ってしまった");
    assert!(!escape.join("x.yml").exists(), "symlink 越しに書かれている");
}

#[test]
fn テンプレート宣言はlistとcheckに種別つきで出る() {
    let sb = Sandbox::new("tpl-meta");
    let cp = sb.dir.join("nonexistent").display().to_string();
    sb.write_file(
        "ok.tpl",
        "a: {{ with secret \"secret/data/x\" }}{{ .Data.data.a }}{{ .Data.data.b }}{{ end }}\n",
    );
    sb.write_file(
        ".config/haj/config",
        &format!(
            "user.secret.KEY1 = vault://secret/data/x/a\nuser.template.OK = {}\nuser.template.MISSING = /nonexistent.tpl\n",
            sb.dir.join("ok.tpl").display()
        ),
    );

    // list: KEY=template:<パス> の形
    let out = sb.haj(&cp, &["secret", "list"], &[]);
    assert!(out.status.success());
    let s = stdout(&out);
    assert!(
        s.contains("KEY1=vault://secret/data/x/a") && s.contains("OK=template:"),
        "種別が判らない:\n{s}"
    );

    // check: 存在と構文を検証(参照の個数つき)。壊れは ✗ で exit 1。金庫に触らない
    let out = sb.haj(&cp, &["secret", "check"], &[("USER", "alice")]);
    assert_eq!(out.status.code(), Some(1), "MISSING があるのに exit 0");
    let s = stdout(&out);
    assert!(
        s.contains("テンプレート (user.template.*)")
            && s.contains("(2 個の参照)")
            && s.contains("✗ /nonexistent.tpl"),
        "テンプレートの検証が出ない:\n{s}"
    );
    assert!(
        !sb.dir.join("vault-args").exists(),
        "check が金庫に触っている"
    );

    // 壊れた tpl は構文エラーとして見える
    sb.write_file("broken.tpl", "x: {{ printf \"nope\" }}\n");
    sb.write_file(
        ".config/haj/config",
        &format!(
            "user.template.B = {}\n",
            sb.dir.join("broken.tpl").display()
        ),
    );
    let out = sb.haj(&cp, &["secret", "check"], &[("USER", "alice")]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stdout(&out).contains("✗") && stdout(&out).contains("解釈できません"),
        "構文エラーが出ない:\n{}",
        stdout(&out)
    );
}

// ---- 0.40.0: 自動ログインの連鎖 — cert 委譲の段 (SPEC §10.4) ----

/// 連鎖検証用の偽 vault。token lookup は logged-in マーカーが在るときだけ成功し、
/// login (OIDC 段) はマーカーを作って記録する。
fn fake_chain_vault(sb: &Sandbox) -> PathBuf {
    let d = sb.dir.display().to_string();
    sb.exe(
        "bin/vault",
        &format!(
            r#"#!/bin/sh
d="{d}"
printf '%s\n' "$*" >> "$d/vault-calls"
case "$1" in
  token) [ -e "$d/logged-in" ]; exit $? ;;
  login) echo "$@" >> "$d/login-called"; touch "$d/logged-in"; exit 0 ;;
  kv) printf 's3cr3t\n'; exit 0 ;;
esac
exit 0
"#
        ),
    )
}

/// 偽の cert 委譲コマンド。呼ばれた証拠と VAULT_ADDR を記録し、ok なら認証を通す。
fn fake_cert(sb: &Sandbox, name: &str, ok: bool) -> PathBuf {
    let d = sb.dir.display().to_string();
    let body = if ok {
        format!("#!/bin/sh\necho \"addr=$VAULT_ADDR\" >> \"{d}/cert-called\"\ntouch \"{d}/logged-in\"\nexit 0\n")
    } else {
        format!("#!/bin/sh\necho \"addr=$VAULT_ADDR\" >> \"{d}/cert-called\"\nexit 1\n")
    };
    sb.exe(&format!("bin/{name}"), &body)
}

#[test]
fn cert委譲が成功すればoidcに進まず解決できる() {
    let sb = Sandbox::new("cert-ok");
    let vault = fake_chain_vault(&sb);
    let cert = fake_cert(&sb, "cert-ok", true);
    let cp = sb.show_command();
    sb.write_file(
        ".config/haj/config",
        &format!(
            "secrets.vault_cert_login = {}\nsecrets.vault_login = -method=oidc\nsecrets.vault_addr = https://vault.example.com\n",
            cert.display()
        ),
    );

    let out = sb.haj(
        &cp,
        &["--secret", "HAJ_T_VALUE=vault://secret/data/x/t", "show"],
        &[("HAJ_VAULT_CMD", vault.to_str().unwrap())],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "s3cr3t");
    let cert_log = read(&sb, "cert-called");
    assert!(
        cert_log.contains("addr=https://vault.example.com"),
        "委譲先に VAULT_ADDR が渡っていない: {cert_log}"
    );
    assert!(
        !sb.dir.join("login-called").exists(),
        "cert 成功なのに OIDC まで進んでいる"
    );
}

#[test]
fn cert委譲が失敗したら静かにoidcの段へ進む() {
    let sb = Sandbox::new("cert-fallback");
    let vault = fake_chain_vault(&sb);
    let cert = fake_cert(&sb, "cert-fail", false);
    let cp = sb.show_command();
    sb.write_file(
        ".config/haj/config",
        &format!(
            "secrets.vault_cert_login = {}\nsecrets.vault_login = -method=oidc\n",
            cert.display()
        ),
    );

    let out = sb.haj(
        &cp,
        &["--secret", "HAJ_T_VALUE=vault://secret/data/x/t", "show"],
        &[("HAJ_VAULT_CMD", vault.to_str().unwrap())],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "s3cr3t");
    assert!(
        sb.dir.join("cert-called").exists(),
        "cert 段が呼ばれていない"
    );
    assert!(
        read(&sb, "login-called").contains("-method=oidc"),
        "OIDC の段に進んでいない"
    );
    assert!(
        stderr(&out).contains("cert 認証は成功しませんでした"),
        "静かな一行が無い: {}",
        stderr(&out)
    );
}

#[test]
fn cert未設定なら段をスキップして従来どおりoidcだけ走る() {
    let sb = Sandbox::new("cert-skip");
    let vault = fake_chain_vault(&sb);
    let cp = sb.show_command();
    sb.write_file(".config/haj/config", "secrets.vault_login = -method=oidc\n");

    let out = sb.haj(
        &cp,
        &["--secret", "HAJ_T_VALUE=vault://secret/data/x/t", "show"],
        &[("HAJ_VAULT_CMD", vault.to_str().unwrap())],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(!sb.dir.join("cert-called").exists());
    assert!(sb.dir.join("login-called").exists(), "OIDC が走っていない");
    assert!(
        !stderr(&out).contains("cert 認証"),
        "未設定なのに cert 段の表示がある: {}",
        stderr(&out)
    );
}

#[test]
fn storeログインも連鎖でstatusは連鎖の設定を出す() {
    let sb = Sandbox::new("cert-store");
    let vault = fake_chain_vault(&sb);
    let cert = fake_cert(&sb, "cert-ok", true);
    let cp = sb.dir.join("nonexistent").display().to_string();
    sb.write_file(
        ".config/haj/config",
        &format!(
            "secrets.vault_cert_login = {}\nsecrets.vault_login = -method=oidc\n",
            cert.display()
        ),
    );
    let envs = [("HAJ_VAULT_CMD", vault.to_str().unwrap())];

    // status: 連鎖の設定が見える(未ログイン時 exit 1)
    let out = sb.haj(&cp, &["store", "status"], &envs);
    assert_eq!(out.status.code(), Some(1));
    let s = stdout(&out);
    assert!(
        s.contains("cert委譲") && s.contains("cert-ok") && s.contains("-method=oidc"),
        "連鎖の設定が出ない:\n{s}"
    );

    // login: cert 段が先に走り、成功したら OIDC に進まない
    let out = sb.haj(&cp, &["store", "login"], &envs);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(
        stderr(&out).contains("cert 認証でログインしました"),
        "cert 成功の表示が無い: {}",
        stderr(&out)
    );
    assert!(!sb.dir.join("login-called").exists(), "OIDC まで進んでいる");
}
