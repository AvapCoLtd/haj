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
fn op参照はinjectに委譲され埋め込みも展開される() {
    let sb = Sandbox::new("op");
    let cp = sb.show_command();
    let op = sb.fake_op();

    // 値全体ではなく埋め込みでも、op だけは inject の意味論に従って展開される
    let out = sb.haj(
        &cp,
        &["show"],
        &[
            ("HAJ_SECRETS", "1"),
            ("HAJ_OP_CMD", op.to_str().unwrap()),
            ("HAJ_T_VALUE", "Bearer op://Infra/ci/token"),
        ],
    );
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert_eq!(stdout(&out).trim(), "Bearer RESOLVED");
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
