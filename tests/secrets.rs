//! シークレット参照の展開(SPEC.md §10)を外から確かめる。
//!
//! 金庫には触らない。偽の vault / op を置き、HAJ_VAULT_CMD / HAJ_OP_CMD で
//! 差し替えて全経路を通す。

use std::fs;
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
fn 展開はオプトインで既定では何もしない() {
    let sb = Sandbox::new("optin");
    let cp = sb.show_command();

    // HAJ_SECRETS が無ければ、参照はただの文字列としてそのまま渡る
    let out = sb.haj(
        &cp,
        &["show"],
        &[("HAJ_T_VALUE", "env://HAJ_T_SRC"), ("HAJ_T_SRC", "hello")],
    );
    assert!(out.status.success());
    assert_eq!(stdout(&out).trim(), "env://HAJ_T_SRC");
}

#[test]
fn env参照は別の環境変数の値になる() {
    let sb = Sandbox::new("env");
    let cp = sb.show_command();

    let out = sb.haj(
        &cp,
        &["show"],
        &[
            ("HAJ_SECRETS", "1"),
            ("HAJ_T_VALUE", "env://HAJ_T_SRC"),
            ("HAJ_T_SRC", "hello"),
        ],
    );
    assert!(out.status.success());
    assert_eq!(stdout(&out).trim(), "hello");
}

#[test]
fn file参照はファイルの中身になり末尾の改行を落とす() {
    let sb = Sandbox::new("file");
    let cp = sb.show_command();
    let f = sb.dir.join("cred");
    fs::write(&f, "t0ps3cret\n").unwrap();

    let out = sb.haj(
        &cp,
        &["show"],
        &[
            ("HAJ_SECRETS", "1"),
            ("HAJ_T_VALUE", &format!("file://{}", f.display())),
        ],
    );
    assert!(out.status.success());
    assert_eq!(stdout(&out), "t0ps3cret\n"); // 中身の改行は show が付けた1つだけ
}

#[test]
fn vault_uri形は最後のセグメントがフィールド() {
    let sb = Sandbox::new("vault-uri");
    let cp = sb.show_command();
    let vault = sb.fake_vault();

    let out = sb.haj(
        &cp,
        &["show"],
        &[
            ("HAJ_SECRETS", "1"),
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("HAJ_T_VALUE", "vault://avap/data/hoge/fuga"),
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "s3cr3t");

    // /data/ 入りパス(template の書き方)は mount と相対パスに読み替える
    let args = fs::read_to_string(sb.dir.join("vault-args")).unwrap();
    assert_eq!(args, "kv\nget\n-field=fuga\n-mount=avap\nhoge\n");
}

#[test]
fn vault_template正準形はuri形と同じ解決になる() {
    let sb = Sandbox::new("vault-tpl");
    let cp = sb.show_command();
    let vault = sb.fake_vault();

    let out = sb.haj(
        &cp,
        &["show"],
        &[
            ("HAJ_SECRETS", "1"),
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            (
                "HAJ_T_VALUE",
                r#"{{ with secret "avap/data/hoge" }}{{ .Data.data.fuga }}{{ end }}"#,
            ),
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "s3cr3t");

    let args = fs::read_to_string(sb.dir.join("vault-args")).unwrap();
    assert_eq!(args, "kv\nget\n-field=fuga\n-mount=avap\nhoge\n");
}

#[test]
fn vault_templateの正準形以外は中止する() {
    let sb = Sandbox::new("vault-canon");
    let cp = sb.mark_command();

    let out = sb.haj(
        &cp,
        &["mark"],
        &[("HAJ_SECRETS", "1"), ("HAJ_T_VALUE", r#"{{ printf "x" }}"#)],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(stderr(&out).contains("正準形"), "stderr: {}", stderr(&out));
    assert!(!sb.dir.join("ran").exists(), "本体が実行されてしまった");
}

#[test]
fn 解決に失敗したら本体を実行しない() {
    let sb = Sandbox::new("failfast");
    let cp = sb.mark_command();
    let vault = sb.exe("bin/vault", "#!/bin/sh\nexit 1\n");

    let out = sb.haj(
        &cp,
        &["mark"],
        &[
            ("HAJ_SECRETS", "1"),
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("HAJ_T_VALUE", "vault://avap/data/hoge/fuga"),
        ],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(!sb.dir.join("ran").exists(), "本体が実行されてしまった");
}

#[test]
fn 引数の参照も展開される() {
    let sb = Sandbox::new("argv");
    let cp = sb.args_command();
    let vault = sb.fake_vault();

    let out = sb.haj(
        &cp,
        &["args", "--token", "vault://avap/data/hoge/fuga", "plain"],
        &[
            ("HAJ_SECRETS", "1"),
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out), "--token\ns3cr3t\nplain\n");
}

#[test]
fn op参照は値全体なら環境変数でも展開される() {
    let sb = Sandbox::new("op");
    let cp = sb.show_command();
    let op = sb.fake_op();

    let out = sb.haj(
        &cp,
        &["show"],
        &[
            ("HAJ_SECRETS", "1"),
            ("HAJ_OP_CMD", op.to_str().unwrap()),
            ("HAJ_T_VALUE", "op://Infra/ci/token"),
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "RESOLVED");
}

#[test]
fn 引数のop参照は埋め込みでもinjectで展開される() {
    let sb = Sandbox::new("op-argv");
    let cp = sb.args_command();
    let op = sb.fake_op();

    // argv は人が明示的に書いたもの。inject の意味論(埋め込み展開)のまま
    let out = sb.haj(
        &cp,
        &["args", "Bearer op://Infra/ci/token"],
        &[("HAJ_SECRETS", "1"), ("HAJ_OP_CMD", op.to_str().unwrap())],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "Bearer RESOLVED");
}

#[test]
fn 参照をたまたま文中に含む環境変数では止まらない() {
    let sb = Sandbox::new("ci-desc");
    let cp = sb.show_command();

    // GitLab の MR パイプラインが CI_MERGE_REQUEST_DESCRIPTION に op:// の例文入りの
    // 説明文を入れてくる、という実際に踏んだ事故の回帰テスト。
    // 偽 op すら置かない: 解決しに行けば「op が見つかりません」で落ちるはず。
    let note = "説明文の途中に op://Infra/ci/token が書いてあるだけ";
    let out = sb.haj(
        &cp,
        &["show"],
        &[("HAJ_SECRETS", "1"), ("HAJ_T_VALUE", note)],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), note); // 触らずそのまま渡る
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
        format!("vault_cmd = {}\n", vault.display()),
    )
    .unwrap();

    let out = sb.haj(
        &cp,
        &["show"],
        &[
            ("HAJ_SECRETS", "1"),
            ("HAJ_T_VALUE", "vault://avap/data/hoge/fuga"),
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "s3cr3t");
}

#[test]
fn secretsコマンドは展開対象を解決せずに列挙する() {
    let sb = Sandbox::new("dryrun");
    let cp = sb.show_command();

    // HAJ_VAULT_CMD は差し替えない。解決しに行けば(vault が無いので)失敗するが、
    // dry-run は金庫に触らないので成功する。
    let out = sb.haj(
        &cp,
        &["secrets"],
        &[
            ("HAJ_SECRETS", "1"),
            ("HAJ_T_VALUE", "vault://avap/data/hoge/fuga"),
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("HAJ_T_VALUE"), "対象の変数名が出ていない:\n{s}");
    assert!(
        s.contains("vault://avap/data/hoge/fuga"),
        "参照の対象が出ていない:\n{s}"
    );
}

#[test]
fn 規約フックには展開しない() {
    let sb = Sandbox::new("hook");
    // --haj-describe が HAJ_T_VALUE をそのまま説明文として返すコマンド。
    // フックの経路で展開されるなら、ここに展開後の値が現れてしまう。
    sb.exe(
        "sys/commands/leaky",
        "#!/bin/sh\ncase \"$1\" in --haj-describe) printf '%s\\n' \"$HAJ_T_VALUE\"; exit 0 ;; --haj-*) exit 0 ;; esac\n",
    );
    let cp = sb.dir.join("sys/commands");

    let out = sb.haj(
        cp.to_str().unwrap(),
        &["commands"],
        &[
            ("HAJ_SECRETS", "1"),
            ("HAJ_T_VALUE", "env://HAJ_T_SRC"),
            ("HAJ_T_SRC", "expanded"),
        ],
    );
    assert!(out.status.success());
    let s = stdout(&out);
    assert!(
        s.contains("env://HAJ_T_SRC"),
        "フックに展開が漏れている:\n{s}"
    );
    assert!(!s.contains("expanded"), "フックに展開が漏れている:\n{s}");
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

const LOGIN_ARGS: &str = "-method=oidc -path=id-avap-keycloak role=direct callbackmode=direct";

#[test]
fn 未ログインなら既定の引数で自動ログインしてから解決する() {
    let sb = Sandbox::new("autologin");
    let cp = sb.show_command();
    let vault = stateful_vault(&sb);

    // vault_login は何も設定しない → 既定の avap OIDC でログインする
    let out = sb.haj(
        &cp,
        &["show"],
        &[
            ("HAJ_SECRETS", "1"),
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("HAJ_T_VALUE", "vault://avap/data/hoge/fuga"),
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "s3cr3t");

    let args = fs::read_to_string(sb.dir.join("login-args")).unwrap();
    assert_eq!(args.trim(), "-method=oidc -path=id-avap-keycloak");

    // サーバの既定も CLI に渡っている
    let addr = fs::read_to_string(sb.dir.join("seen-addr")).unwrap();
    assert_eq!(addr.trim(), "https://vault.avap.plus");
}

#[test]
fn vault_loginの設定が既定の引数を上書きする() {
    let sb = Sandbox::new("loginargs");
    let cp = sb.show_command();
    let vault = stateful_vault(&sb);

    let out = sb.haj(
        &cp,
        &["show"],
        &[
            ("HAJ_SECRETS", "1"),
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("HAJ_VAULT_LOGIN", LOGIN_ARGS),
            ("HAJ_T_VALUE", "vault://avap/data/hoge/fuga"),
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
        &["mark"],
        &[
            ("HAJ_SECRETS", "1"),
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("HAJ_VAULT_LOGIN", "off"),
            ("HAJ_T_VALUE", "vault://avap/data/hoge/fuga"),
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
        &["show"],
        &[
            ("HAJ_SECRETS", "1"),
            ("HAJ_VAULT_CMD", vault.to_str().unwrap()),
            ("HAJ_VAULT_LOGIN", LOGIN_ARGS),
            ("HAJ_T_VALUE", "vault://avap/data/hoge/fuga"),
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "s3cr3t");
    assert!(
        !sb.dir.join("login-args").exists(),
        "ログイン済みなのにloginが走った"
    );
}

// ---- 明示的な受け渡し(SPEC §10.7): --secret / --env / --secretfile ----

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
            "HAJ_T_VALUE=vault://avap/data/hoge/fuga",
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
        "HAJ_T_VALUE = vault://avap/data/hoge/fuga\nHAJ_T_NOTE = 文中の op://x はただの文字列\n",
    );

    let out = sb.haj(
        &cp,
        &["--env", "mig.env", "show"],
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
        &["--env", "a.env", "--secret", "HAJ_T_VALUE=あとの値", "show"],
        &[],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "あとの値");
}

#[test]
fn secretfileはテンプレートを描画して0600で書く() {
    let sb = Sandbox::new("flag-file");
    let cp = sb.mark_command();
    let vault = sb.fake_vault();
    let op = sb.fake_op();
    sb.write_file(
        "config.ini.tpl",
        "[db]\npassword = {{ with secret \"avap/data/hoge\" }}{{ .Data.data.fuga }}{{ end }}\ntoken = op://Infra/ci/token\n",
    );

    let out = sb.haj(
        &cp,
        &["--secretfile", "config.ini=config.ini.tpl", "mark"],
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
fn secretfileの解決に失敗したら書かずに止まる() {
    let sb = Sandbox::new("flag-file-fail");
    let cp = sb.mark_command();
    let vault = sb.exe("bin/vault", "#!/bin/sh\nexit 1\n");
    sb.write_file(
        "bad.tpl",
        "x = {{ with secret \"avap/data/hoge\" }}{{ .Data.data.fuga }}{{ end }}\n",
    );

    let out = sb.haj(
        &cp,
        &["--secretfile", "out.ini=bad.tpl", "mark"],
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
            "HAJ_T_VALUE=vault://avap/data/hoge/fuga",
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
            "HAJ_T_VALUE=vault://avap/data/hoge/fuga",
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
        "token = vault://users/hajime/gitlab-pat/gitlab.avaper.day/token\n",
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
        "token = vault://users/hajime/gitlab-pat/gitlab.avaper.day/token\n",
    )
    .unwrap();

    let out = sb.haj(&cp, &["config"], &[("HAJ_TOKEN", "")]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    let s = stdout(&out);
    assert!(
        s.contains("vault://users/hajime/gitlab-pat/gitlab.avaper.day/token"),
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
        "op_cmd",
        "vault_cmd",
        "vault_addr",
        "vault_login",
        "gitlab",
        "project_id",
        "target",
        "token",
    ] {
        assert!(
            s.contains(&format!("# {key} ")) || s.contains(&format!("# {key} =")),
            "{key} が雛形に無い:\n{s}"
        );
    }
    // 既定値も出ている
    assert!(s.contains("https://vault.avap.plus"), "既定値が無い:\n{s}");
    assert!(
        s.contains("-method=oidc -path=id-avap-keycloak"),
        "既定値が無い:\n{s}"
    );

    // 全行コメントか空行 = そのまま置いても挙動が変わらない
    for line in s.lines() {
        assert!(
            line.is_empty() || line.starts_with('#'),
            "コメントでない行がある: {line}"
        );
    }
}
