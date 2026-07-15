//! hajの振る舞いを、実際にサブコマンドを置いて外から確かめる。
//!
//! hajの本質は「探索」と「規約」なので、内部関数の単体テストより、
//! 一時ディレクトリに本物の実行ファイルを置いて叩くほうが意味がある。

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// テストごとに独立した作業ディレクトリを作る。
/// (std だけで書くので tempfile は使わない)
struct Sandbox {
    dir: PathBuf,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("haj-test-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    /// 実行可能なサブコマンドを置く。
    fn command(&self, tree: &str, name: &str, body: &str) -> PathBuf {
        let dir = self.dir.join(tree).join("commands");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn write(&self, rel: &str, body: &str) {
        let path = self.dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.join(rel)
    }

    /// hajを走らせる。cwd と HAJ_COMMAND_PATH を明示する。
    fn haj(&self, cwd: &Path, command_path: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_haj"))
            .args(args)
            .current_dir(cwd)
            .env("HAJ_COMMAND_PATH", command_path)
            .env("HAJ_NO_CACHE", "1") // テスト間でキャッシュを共有しない
            .env("HOME", &self.dir) // ユーザーの設定を汚さない
            .output()
            .unwrap()
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

/// 規約に従うサブコマンドの雛形。
fn conforming(desc: &str, help: &str, complete: &str, body: &str) -> String {
    format!(
        r#"#!/bin/sh
case "$1" in
  --haj-describe) echo "{desc}"; exit 0 ;;
  --haj-help)     echo "{help}"; exit 0 ;;
  --haj-complete) shift; [ $# -eq 0 ] && printf '%s\n' {complete}; exit 0 ;;
esac
{body}
"#
    )
}

#[test]
fn システム共通のコマンドを見つけて実行する() {
    let sb = Sandbox::new("sys");
    sb.command(
        "sys",
        "greet",
        &conforming("あいさつ", "使い方: greet", "a b", "echo hello $1"),
    );

    let cp = sb.path("sys/commands");
    let out = sb.haj(&sb.dir, cp.to_str().unwrap(), &["greet", "world"]);

    assert!(out.status.success());
    assert_eq!(stdout(&out).trim(), "hello world");
}

#[test]
fn 説明文を全コマンドに聞いて一覧を自動生成する() {
    let sb = Sandbox::new("list");
    sb.command(
        "sys",
        "alpha",
        &conforming("最初のコマンド", "", "", "true"),
    );
    sb.command("sys", "beta", &conforming("次のコマンド", "", "", "true"));

    let cp = sb.path("sys/commands");
    let out = sb.haj(&sb.dir, cp.to_str().unwrap(), &["help"]);
    let s = stdout(&out);

    assert!(s.contains("alpha"), "一覧にalphaが無い:\n{s}");
    assert!(s.contains("最初のコマンド"), "説明が出ていない:\n{s}");
    assert!(s.contains("beta"));
    assert!(s.contains("次のコマンド"));
}

#[test]
fn プロジェクト固有のコマンドはそのリポジトリの中でだけ生える() {
    let sb = Sandbox::new("proj");
    sb.command("sys", "shared", &conforming("共通コマンド", "", "", "true"));
    // proj/.haj/commands/deploy — projディレクトリの中でだけ見えるべき
    sb.command(
        "proj/.haj",
        "deploy",
        &conforming("このリポジトリ専用", "", "", "true"),
    );

    let cp = sb.path("sys/commands");
    let cp = cp.to_str().unwrap();

    let inside = stdout(&sb.haj(&sb.path("proj"), cp, &["__complete"]));
    assert!(
        inside.contains("deploy"),
        "リポジトリ内でdeployが見えない:\n{inside}"
    );
    assert!(
        inside.contains("shared"),
        "共通コマンドも見えるべき:\n{inside}"
    );

    let outside = stdout(&sb.haj(&sb.dir, cp, &["__complete"]));
    assert!(
        !outside.contains("deploy"),
        "リポジトリ外にdeployが漏れている:\n{outside}"
    );
    assert!(outside.contains("shared"));
}

#[test]
fn 同名ならプロジェクト固有がシステム共通に勝つ() {
    let sb = Sandbox::new("shadow");
    sb.command(
        "sys",
        "build",
        &conforming("共通のbuild", "", "", "echo SYSTEM"),
    );
    sb.command(
        "proj/.haj",
        "build",
        &conforming("このリポジトリのbuild", "", "", "echo PROJECT"),
    );

    let cp = sb.path("sys/commands");
    let cp = cp.to_str().unwrap();

    let inside = sb.haj(&sb.path("proj"), cp, &["build"]);
    assert_eq!(
        stdout(&inside).trim(),
        "PROJECT",
        "プロジェクト側が勝つべき"
    );

    let outside = sb.haj(&sb.dir, cp, &["build"]);
    assert_eq!(stdout(&outside).trim(), "SYSTEM");
}

#[test]
fn 親ディレクトリのhajも見つかる() {
    let sb = Sandbox::new("ancestor");
    sb.command(
        "proj/.haj",
        "deploy",
        &conforming("デプロイ", "", "", "true"),
    );
    fs::create_dir_all(sb.path("proj/deep/nested")).unwrap();

    let cp = sb.path("nonexistent");
    // リポジトリの深いところで打っても、上に遡って .haj/commands が見つかる
    let out = stdout(&sb.haj(
        &sb.path("proj/deep/nested"),
        cp.to_str().unwrap(),
        &["__complete"],
    ));
    assert!(
        out.contains("deploy"),
        "祖先の.hajが見つかっていない:\n{out}"
    );
}

#[test]
fn haj_rootを渡すので共通ライブラリを解決できる() {
    let sb = Sandbox::new("root");
    sb.command(
        "sys",
        "show",
        "#!/bin/sh\necho \"root=$HAJ_ROOT name=$HAJ_NAME\"\n",
    );

    let cp = sb.path("sys/commands");
    let out = stdout(&sb.haj(&sb.dir, cp.to_str().unwrap(), &["show"]));

    assert_eq!(
        out.trim(),
        format!("root={} name=show", sb.path("sys").display())
    );
}

#[test]
fn 補完はコマンド自身に聞く() {
    let sb = Sandbox::new("complete");
    sb.command(
        "sys",
        "mig",
        r#"#!/bin/sh
case "$1" in
  --haj-describe) echo "マイグレーション"; exit 0 ;;
  --haj-complete)
    shift
    # 入力済みの語が0語なら操作を、1語以上ならスキーマを返す
    if [ $# -eq 0 ]; then printf '%s\n' up down; else printf '%s\n' v0 v2; fi
    exit 0 ;;
esac
"#,
    );

    let cp = sb.path("sys/commands");
    let cp = cp.to_str().unwrap();

    let first = stdout(&sb.haj(&sb.dir, cp, &["__complete", "mig"]));
    assert_eq!(first.split_whitespace().collect::<Vec<_>>(), ["up", "down"]);

    // `haj mig up <TAB>` — 入力済みは1語なので、次はスキーマ
    let second = stdout(&sb.haj(&sb.dir, cp, &["__complete", "mig", "up"]));
    assert_eq!(second.split_whitespace().collect::<Vec<_>>(), ["v0", "v2"]);
}

#[test]
fn 規約に応答しないコマンドも実行はできる() {
    let sb = Sandbox::new("naive");
    // --haj-describe を知らない素朴なスクリプト。説明は空になるが、動くべき。
    sb.command("sys", "plain", "#!/bin/sh\necho ran\n");

    let cp = sb.path("sys/commands");
    let cp = cp.to_str().unwrap();

    assert_eq!(stdout(&sb.haj(&sb.dir, cp, &["plain"])).trim(), "ran");

    // 一覧には出る(説明が空なだけ)
    let list = stdout(&sb.haj(&sb.dir, cp, &["help"]));
    assert!(
        list.contains("plain"),
        "規約非対応でも一覧には出すべき:\n{list}"
    );
}

#[test]
fn 固まるコマンドがhelpを巻き添えにしない() {
    let sb = Sandbox::new("hang");
    sb.command(
        "sys",
        "good",
        &conforming("まともなコマンド", "", "", "true"),
    );
    // --haj-describe で永久に固まる壊れたコマンド
    sb.command("sys", "hang", "#!/bin/sh\nsleep 300\n");

    let cp = sb.path("sys/commands");
    let out = Command::new(env!("CARGO_BIN_EXE_haj"))
        .args(["help"])
        .current_dir(&sb.dir)
        .env("HAJ_COMMAND_PATH", cp.to_str().unwrap())
        .env("HAJ_NO_CACHE", "1")
        .env("HOME", &sb.dir)
        .env("HAJ_HOOK_TIMEOUT_MS", "300") // テストを待たせない
        .output()
        .unwrap();

    let s = stdout(&out);
    assert!(out.status.success(), "helpが失敗した");
    assert!(s.contains("good"), "まともなコマンドは出るべき:\n{s}");
    assert!(s.contains("hang"), "固まるコマンドも名前だけは出す:\n{s}");
}

#[test]
fn 終了コードはサブコマンドのものがそのまま伝わる() {
    let sb = Sandbox::new("exit");
    sb.command("sys", "fail", "#!/bin/sh\nexit 42\n");

    let cp = sb.path("sys/commands");
    let out = sb.haj(&sb.dir, cp.to_str().unwrap(), &["fail"]);
    assert_eq!(out.status.code(), Some(42));
}

#[test]
fn 未知のコマンドは127で終わる() {
    let sb = Sandbox::new("unknown");
    let cp = sb.path("sys/commands");
    let out = sb.haj(&sb.dir, cp.to_str().unwrap(), &["nosuch"]);

    assert_eq!(
        out.status.code(),
        Some(127),
        "シェルのcommand not foundに揃える"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("未知のコマンド"), "{err}");
}

#[test]
fn shebangのインタプリタが無いときは原因を教える() {
    let sb = Sandbox::new("shebang");
    // 存在しないインタプリタを指すコマンド。カーネルはこれを ENOENT で返すため、
    // 素のメッセージは「No such file or directory」になり、ファイルはそこに在るのに
    // 何が無いのか分からない。hajはshebangを読んで補足すべき。
    sb.command(
        "sys",
        "broken",
        "#!/nonexistent/interpreter\necho unreachable\n",
    );

    let cp = sb.path("sys/commands");
    let out = sb.haj(&sb.dir, cp.to_str().unwrap(), &["broken"]);

    assert_eq!(out.status.code(), Some(126), "見つかったが実行できない");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("/nonexistent/interpreter"),
        "shebangのインタプリタを指摘していない:\n{err}"
    );
}

// --- プロジェクト境界と素性 --------------------------------------------------
//
// 探索を素朴に / まで遡って全部積むと、上流の野良 .haj が黙って効いてしまい、
// しかも「どのプロジェクトの setup が走ったのか」が分からない。
// setup/reset は破壊的なので、これは事故になる。

#[test]
fn 上流の野良hajは境界で遮られる() {
    let sb = Sandbox::new("boundary");
    // 誰かが上流(リポジトリの親)に置いてしまった setup
    sb.command(
        "upstream/.haj",
        "setup",
        &conforming("野良setup", "", "", "echo LEAKED"),
    );
    // 自分のプロジェクト。.haj があるのでここが境界になる
    sb.command(
        "upstream/myproj/.haj",
        "build",
        &conforming("自分のbuild", "", "", "true"),
    );

    let cp = sb.path("none");
    let out = stdout(&sb.haj(
        &sb.path("upstream/myproj"),
        cp.to_str().unwrap(),
        &["__complete"],
    ));

    assert!(out.contains("build"), "自分のコマンドが見えない:\n{out}");
    assert!(
        !out.contains("setup"),
        "上流の野良setupが境界を越えて漏れている:\n{out}"
    );
}

#[test]
fn root_falseなら親の共通コマンドも継承する() {
    let sb = Sandbox::new("monorepo");
    // モノレポのルート。共通コマンド mig を持つ
    sb.command("mono/.haj", "mig", &conforming("共通のmig", "", "", "true"));
    // サブプロジェクト。root = false と宣言して親も見に行く
    sb.command(
        "mono/web/.haj",
        "setup",
        &conforming("webのsetup", "", "", "true"),
    );
    sb.write("mono/web/.haj/config", "name = web\nroot = false\n");

    let cp = sb.path("none");
    let out = stdout(&sb.haj(&sb.path("mono/web"), cp.to_str().unwrap(), &["__complete"]));

    assert!(out.contains("setup"), "自分のsetupが無い:\n{out}");
    assert!(
        out.contains("mig"),
        "root=falseなのに親のmigを継承していない:\n{out}"
    );
}

#[test]
fn どのプロジェクトのコマンドかを一覧に出す() {
    let sb = Sandbox::new("origin");
    sb.command(
        "sys",
        "bao-login",
        &conforming("Vaultログイン", "", "", "true"),
    );
    sb.command(
        "proj/.haj",
        "setup",
        &conforming("セットアップ", "", "", "true"),
    );
    sb.write("proj/.haj/config", "name = example-app\n");

    let cp = sb.path("sys/commands");
    let out = stdout(&sb.haj(&sb.path("proj"), cp.to_str().unwrap(), &["help"]));

    assert!(
        out.contains("プロジェクト: example-app"),
        "いまどのプロジェクトにいるのかを言っていない:\n{out}"
    );
    assert!(
        out.contains("[example-app]"),
        "setupの出自が出ていない:\n{out}"
    );
    assert!(
        out.contains("[共通]"),
        "共通コマンドの出自が出ていない:\n{out}"
    );
}

#[test]
fn プロジェクト名は既定でディレクトリ名になる() {
    let sb = Sandbox::new("defaultname");
    sb.command(
        "myrepo/.haj",
        "setup",
        &conforming("セットアップ", "", "", "true"),
    );
    // .haj/config を置いていない

    let cp = sb.path("none");
    let out = stdout(&sb.haj(&sb.path("myrepo"), cp.to_str().unwrap(), &["help"]));

    assert!(
        out.contains("プロジェクト: myrepo"),
        "ディレクトリ名がプロジェクト名にならない:\n{out}"
    );
}

// --- コア組み込みコマンド ------------------------------------------------------
//
// help / commands / which / selfupgrade はどこにいても使える。探索の対象では
// ないからといって一覧や補完から漏らすと、「haj help の一覧が実態と一致する」
// という haj の約束そのものが嘘になる。

#[test]
fn 組み込みコマンドは常に一覧に出る() {
    let sb = Sandbox::new("builtin-list");
    sb.command(
        "sys",
        "mig",
        &conforming("マイグレーション", "", "", "true"),
    );

    let cp = sb.path("sys/commands");
    let out = stdout(&sb.haj(&sb.dir, cp.to_str().unwrap(), &["help"]));

    for b in ["help", "commands", "which", "selfupgrade"] {
        assert!(out.contains(b), "組み込み {b} が一覧に無い:\n{out}");
    }
    assert!(out.contains("mig"), "探索されたコマンドも出るべき:\n{out}");
}

#[test]
fn プロジェクトが空でも組み込みは出る() {
    let sb = Sandbox::new("builtin-empty");
    // コマンドが1つも無い状態
    let cp = sb.path("none");
    let out = stdout(&sb.haj(&sb.dir, cp.to_str().unwrap(), &["help"]));

    assert!(
        out.contains("selfupgrade"),
        "コマンドが無くても組み込みは使えるのだから出すべき:\n{out}"
    );
}

#[test]
fn 組み込みコマンドは補完に出る() {
    let sb = Sandbox::new("builtin-complete");
    sb.command(
        "sys",
        "mig",
        &conforming("マイグレーション", "", "", "true"),
    );

    let cp = sb.path("sys/commands");
    let cp = cp.to_str().unwrap();

    let out = stdout(&sb.haj(&sb.dir, cp, &["__complete"]));
    assert!(out.contains("selfupgrade"), "TABで組み込みが出ない:\n{out}");
    assert!(out.contains("which"), "{out}");
    assert!(out.contains("mig"), "探索されたコマンドも出るべき:\n{out}");

    // haj selfupgrade <TAB> → --check
    let sub = stdout(&sb.haj(&sb.dir, cp, &["__complete", "selfupgrade"]));
    assert!(sub.contains("--check"), "selfupgradeの補完が無い:\n{sub}");

    // haj which <TAB> → --all とコマンド名
    let w = stdout(&sb.haj(&sb.dir, cp, &["__complete", "which"]));
    assert!(w.contains("--all"), "{w}");
    assert!(w.contains("mig"), "whichの引数はコマンド名であるべき:\n{w}");
}

#[test]
fn 組み込みコマンドにも使い方がある() {
    let sb = Sandbox::new("builtin-help");
    let cp = sb.path("none");
    let cp = cp.to_str().unwrap();

    for b in ["help", "commands", "which", "selfupgrade"] {
        let out = stdout(&sb.haj(&sb.dir, cp, &["help", b]));
        assert!(
            out.contains(&format!("haj {b}")),
            "haj help {b} が説明を返さない:\n{out}"
        );
    }
}

#[test]
fn 機械可読の一覧にも組み込みが入る() {
    let sb = Sandbox::new("builtin-commands");
    sb.command(
        "sys",
        "mig",
        &conforming("マイグレーション", "", "", "true"),
    );

    let cp = sb.path("sys/commands");
    let out = stdout(&sb.haj(&sb.dir, cp.to_str().unwrap(), &["commands"]));

    let selfup: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with("selfupgrade\t"))
        .collect();
    assert_eq!(selfup.len(), 1, "commands に selfupgrade が無い:\n{out}");
    assert!(
        selfup[0].contains("[haj]"),
        "出自ラベルが無い:\n{}",
        selfup[0]
    );
}

#[test]
fn 組み込みと同名のコマンドは置いても無視される() {
    let sb = Sandbox::new("builtin-shadow");
    // selfupgrade を乗っ取ろうとするコマンド
    sb.command(
        "sys",
        "selfupgrade",
        &conforming("乗っ取り", "", "", "echo HIJACKED"),
    );

    let cp = sb.path("sys/commands");
    let out = stdout(&sb.haj(&sb.dir, cp.to_str().unwrap(), &["commands"]));

    // 一覧に出るのは組み込みの方だけ(探索側は予約語として弾かれる)
    let rows: Vec<&str> = out
        .lines()
        .filter(|l| l.starts_with("selfupgrade\t"))
        .collect();
    assert_eq!(rows.len(), 1, "予約語が二重に出ている:\n{out}");
    assert!(
        rows[0].contains("[haj]"),
        "組み込みが奪われた:\n{}",
        rows[0]
    );
}

// --- ユーザー設定 (~/.config/haj/) ---------------------------------------------
//
// 場所は XDG に従う。gitと同じ形 — リポジトリ側は .haj/(gitの .git/)、
// ユーザー側は ~/.config/haj/(gitの ~/.config/git/config)。
// 形式は .haj/config と同じ key = value(覚えることを1つに保つ)。

#[test]
fn 個人用コマンドはxdgの下から拾う() {
    let sb = Sandbox::new("xdg-commands");
    // $XDG_CONFIG_HOME/haj/commands
    sb.command(
        "xdgconf/haj",
        "mine",
        &conforming("個人用", "", "", "echo MINE"),
    );

    let cp = sb.path("none");
    let out = Command::new(env!("CARGO_BIN_EXE_haj"))
        .args(["__complete"])
        .current_dir(&sb.dir)
        .env("HAJ_COMMAND_PATH", cp.to_str().unwrap())
        .env("HAJ_NO_CACHE", "1")
        .env("HOME", &sb.dir)
        .env("XDG_CONFIG_HOME", sb.path("xdgconf"))
        .output()
        .unwrap();

    let s = stdout(&out);
    assert!(
        s.contains("mine"),
        "~/.config/haj/commands が読まれていない:\n{s}"
    );
}

#[test]
fn 設定ファイルの値が既定値を上書きする() {
    let sb = Sandbox::new("cfg-file");
    sb.write(
        "xdgconf/haj/config",
        "hook_timeout_ms = 1234\nselfupgrade.gitlab = https://example.test\n",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_haj"))
        .args(["config"])
        .current_dir(&sb.dir)
        .env("HAJ_NO_CACHE", "1")
        .env("HOME", &sb.dir)
        .env("XDG_CONFIG_HOME", sb.path("xdgconf"))
        .env_remove("HAJ_HOOK_TIMEOUT_MS")
        .env_remove("HAJ_GITLAB")
        .output()
        .unwrap();

    let s = stdout(&out);
    assert!(s.contains("1234"), "設定ファイルの値が効いていない:\n{s}");
    assert!(s.contains("https://example.test"), "{s}");
    assert!(s.contains("設定ファイル"), "出所が出ていない:\n{s}");
}

#[test]
fn 環境変数は設定ファイルより強い() {
    let sb = Sandbox::new("cfg-env");
    sb.write(
        "xdgconf/haj/config",
        "selfupgrade.gitlab = https://from-file.test\n",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_haj"))
        .args(["config"])
        .current_dir(&sb.dir)
        .env("HAJ_NO_CACHE", "1")
        .env("HOME", &sb.dir)
        .env("XDG_CONFIG_HOME", sb.path("xdgconf"))
        .env("HAJ_GITLAB", "https://from-env.test")
        .output()
        .unwrap();

    let s = stdout(&out);
    assert!(
        s.contains("https://from-env.test"),
        "環境変数が勝つべき:\n{s}"
    );
    assert!(
        !s.contains("from-file"),
        "設定ファイルの値が残っている:\n{s}"
    );
    assert!(s.contains("環境変数"), "出所が出ていない:\n{s}");
}

#[test]
fn トークンの値は表示しない() {
    let sb = Sandbox::new("cfg-token");
    sb.write(
        "xdgconf/haj/config",
        "selfupgrade.token = glpat-SUPERSECRET\n",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_haj"))
        .args(["config"])
        .current_dir(&sb.dir)
        .env("HAJ_NO_CACHE", "1")
        .env("HOME", &sb.dir)
        .env("XDG_CONFIG_HOME", sb.path("xdgconf"))
        .env_remove("HAJ_TOKEN")
        .output()
        .unwrap();

    let s = stdout(&out);
    assert!(
        !s.contains("SUPERSECRET"),
        "トークンの実体が表示されている(履歴やスクショに残る):\n{s}"
    );
    assert!(s.contains("********"), "設定済みであることは示すべき:\n{s}");
    assert!(s.contains("設定ファイル"), "出所は示すべき:\n{s}");
}

#[test]
fn 設定ファイルのコメントと引用符を扱える() {
    let sb = Sandbox::new("cfg-parse");
    sb.write(
        "xdgconf/haj/config",
        "# これはコメント\nselfupgrade.gitlab = \"https://quoted.test\"   # 行末コメント\n\n",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_haj"))
        .args(["config"])
        .current_dir(&sb.dir)
        .env("HAJ_NO_CACHE", "1")
        .env("HOME", &sb.dir)
        .env("XDG_CONFIG_HOME", sb.path("xdgconf"))
        .env_remove("HAJ_GITLAB")
        .output()
        .unwrap();

    let s = stdout(&out);
    assert!(
        s.contains("https://quoted.test"),
        "引数の引用符/コメントを剥がせていない:\n{s}"
    );
}

#[test]
fn 対象プロジェクトを環境変数で渡す() {
    let sb = Sandbox::new("projenv");
    sb.command(
        "proj/.haj",
        "setup",
        "#!/bin/sh\necho \"project=$HAJ_PROJECT dir=$HAJ_PROJECT_DIR\"\n",
    );
    sb.write("proj/.haj/config", "name = example-app\n");

    let cp = sb.path("none");
    let out = stdout(&sb.haj(&sb.path("proj"), cp.to_str().unwrap(), &["setup"]));

    assert_eq!(
        out.trim(),
        format!("project=example-app dir={}", sb.path("proj").display()),
        "破壊的なコマンドが対象プロジェクトを名乗れない"
    );
}

#[test]
fn which_allで隠れている定義まで見える() {
    let sb = Sandbox::new("whichall");
    sb.command("sys", "setup", &conforming("共通のsetup", "", "", "true"));
    sb.command(
        "proj/.haj",
        "setup",
        &conforming("固有のsetup", "", "", "true"),
    );
    sb.write("proj/.haj/config", "name = myproj\n");

    let cp = sb.path("sys/commands");
    let out = stdout(&sb.haj(
        &sb.path("proj"),
        cp.to_str().unwrap(),
        &["which", "--all", "setup"],
    ));

    let lines: Vec<&str> = out.lines().filter(|l| l.contains("setup")).collect();
    assert_eq!(lines.len(), 2, "候補が2つ出るべき:\n{out}");
    assert!(lines[0].starts_with('*'), "勝っている方に印が無い:\n{out}");
    assert!(
        lines[0].contains("[myproj]"),
        "勝つのはプロジェクト側:\n{out}"
    );
    assert!(
        lines[1].contains("[共通]"),
        "隠れている方が見えない:\n{out}"
    );
}

#[test]
fn 実行ビットが無いファイルはコマンドとして扱わない() {
    let sb = Sandbox::new("noexec");
    let dir = sb.path("sys/commands");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("README.md"), "これはコマンドではない").unwrap();
    sb.command("sys", "real", &conforming("本物", "", "", "true"));

    let out = stdout(&sb.haj(&sb.dir, dir.to_str().unwrap(), &["__complete"]));
    assert!(out.contains("real"));
    assert!(
        !out.contains("README"),
        "実行ビットの無いファイルを拾っている:\n{out}"
    );
}

#[test]
fn helpという名前のコマンドはコアを奪えない() {
    let sb = Sandbox::new("reserved");
    sb.command(
        "sys",
        "help",
        &conforming("乗っ取り", "", "", "echo HIJACKED"),
    );
    sb.command("sys", "real", &conforming("本物", "", "", "true"));

    let cp = sb.path("sys/commands");
    let out = stdout(&sb.haj(&sb.dir, cp.to_str().unwrap(), &["help"]));

    assert!(!out.contains("HIJACKED"), "予約語helpが奪われた:\n{out}");
    assert!(out.contains("real"), "コアのhelpが出るべき:\n{out}");
}

#[test]
fn whichで勝っている定義を確認できる() {
    let sb = Sandbox::new("which");
    sb.command("sys", "build", &conforming("共通", "", "", "true"));
    sb.command("proj/.haj", "build", &conforming("固有", "", "", "true"));

    let cp = sb.path("sys/commands");
    let out = stdout(&sb.haj(&sb.path("proj"), cp.to_str().unwrap(), &["which", "build"]));

    assert_eq!(
        out.trim(),
        sb.path("proj/.haj/commands/build").display().to_string()
    );
}

#[test]
fn ヘッダとフッタを挟んでコマンド一覧を出す() {
    let sb = Sandbox::new("frag");
    sb.command("sys", "real", &conforming("本物", "", "", "true"));
    sb.write("sys/help.header", "=== 先頭の案内 ===\n");
    sb.write("sys/help.footer", "=== 末尾の案内 ===\n");

    let cp = sb.path("sys/commands");
    let out = stdout(&sb.haj(&sb.dir, cp.to_str().unwrap(), &["help"]));

    let head = out.find("先頭の案内").expect("headerが出ていない");
    let body = out.find("real").expect("コマンド一覧が出ていない");
    let foot = out.find("末尾の案内").expect("footerが出ていない");
    assert!(head < body && body < foot, "順序が違う:\n{out}");
}

// ---- haj -C(SPEC §3.2): git と同じく実行ディレクトリを変える ----

#[test]
fn ハイフンcで別ディレクトリを起点に実行する() {
    let sb = Sandbox::new("chdir");
    sb.command(
        "proj/.haj",
        "deploy",
        &conforming("このリポジトリ専用", "", "", "pwd"),
    );

    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    // sb.dir(プロジェクトの外)から -C proj で入る → 探索も cwd も proj 起点
    let out = sb.haj(&sb.dir, cp, &["-C", "proj", "deploy"]);
    assert!(out.status.success(), "-C 越しに見つからない");
    assert_eq!(
        stdout(&out).trim(),
        sb.path("proj")
            .canonicalize()
            .unwrap()
            .display()
            .to_string(),
        "サブコマンドの cwd が移動先になっていない"
    );

    // -C 無しでは見えない(従来どおり)
    let out = sb.haj(&sb.dir, cp, &["deploy"]);
    assert_eq!(out.status.code(), Some(127));
}

#[test]
fn ハイフンcは複数指定で相対に積み重なる() {
    let sb = Sandbox::new("chdir-multi");
    sb.command("a/b/.haj", "inner", &conforming("内側", "", "", "true"));

    let cp = sb.path("nonexistent");
    let out = sb.haj(
        &sb.dir,
        cp.to_str().unwrap(),
        &["-C", "a", "-C", "b", "inner"],
    );
    assert!(out.status.success(), "-C の積み重ねが git と違う");
}

#[test]
fn ハイフンcの移動先が無ければ実行しない() {
    let sb = Sandbox::new("chdir-fail");
    sb.command("sys", "greet", &conforming("あいさつ", "", "", "true"));

    let cp = sb.path("sys/commands");
    let out = sb.haj(
        &sb.dir,
        cp.to_str().unwrap(),
        &["-C", "no-such-dir", "greet"],
    );
    assert_eq!(out.status.code(), Some(1));
    let e = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(e.contains("移動できません"), "stderr: {e}");
}

#[test]
fn helpにグローバルフラグの一覧が出る() {
    let sb = Sandbox::new("help-flags");
    let cp = sb.path("nonexistent");

    let out = sb.haj(&sb.dir, cp.to_str().unwrap(), &["help"]);
    let s = stdout(&out);
    assert!(s.contains("グローバルフラグ"), "節が無い:\n{s}");
    for f in ["-C ", "--secret ", "--env-file ", "--secret-file "] {
        assert!(s.contains(f), "{f} がヘルプに無い:\n{s}");
    }
}

// ---- haj docs(SPEC §9.3): 探索に乗るドキュメント ----

#[test]
fn 同梱ドキュメントはどこでも読める() {
    let sb = Sandbox::new("docs-core");
    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    let out = sb.haj(&sb.dir, cp, &["docs", "writing-commands"]);
    assert!(out.status.success());
    let s = stdout(&out);
    assert!(s.contains("コマンドの作り方"), "同梱docが出ない:\n{s}");
    assert!(s.contains("--haj-describe"), "規約の説明が無い:\n{s}");

    let spec = stdout(&sb.haj(&sb.dir, cp, &["docs", "spec"]));
    assert!(spec.contains("契約バージョン"), "SPECが埋め込まれていない");
}

#[test]
fn ツリーのdocsは探索に乗り一覧に出自と見出しが出る() {
    let sb = Sandbox::new("docs-tree");
    sb.write(
        "proj/.haj/docs/onboarding.md",
        "# 新人向けセットアップ\n\nまず haj setup を打つ。\n",
    );

    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    // プロジェクトの中では見える
    let list = stdout(&sb.haj(&sb.path("proj"), cp, &["docs"]));
    assert!(list.contains("onboarding"), "一覧に出ない:\n{list}");
    assert!(
        list.contains("新人向けセットアップ"),
        "見出しが説明にならない:\n{list}"
    );
    assert!(
        list.contains("writing-commands"),
        "同梱も一覧に出るべき:\n{list}"
    );

    let body = stdout(&sb.haj(&sb.path("proj"), cp, &["docs", "onboarding"]));
    assert!(body.contains("haj setup"), "本文が出ない:\n{body}");

    // プロジェクトの外では見えない(境界はコマンドと同じ)
    let outside = stdout(&sb.haj(&sb.dir, cp, &["docs"]));
    assert!(
        !outside.contains("onboarding"),
        "境界を越えて漏れている:\n{outside}"
    );
}

#[test]
fn docsの一覧は非端末ならfzfがあっても素の印字のまま() {
    // 選択UI(SPEC §9.3)はstdoutが端末(TTY)のときだけ。パイプ(=このテスト)では
    // fzfがPATHに居ても起動せず、従来の一覧を印字する — スクリプト互換の要。
    let sb = Sandbox::new("docs-no-fzf-pipe");
    let bin = sb.path("bin");
    fs::create_dir_all(&bin).unwrap();
    let fake = bin.join("fzf");
    fs::write(
        &fake,
        format!("#!/bin/sh\ntouch '{}/fzf-was-called'\n", sb.dir.display()),
    )
    .unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();

    let cp = sb.path("nonexistent");
    let out = Command::new(env!("CARGO_BIN_EXE_haj"))
        .args(["docs"])
        .current_dir(&sb.dir)
        .env("HAJ_COMMAND_PATH", cp.to_str().unwrap())
        .env("HAJ_NO_CACHE", "1")
        .env("HOME", &sb.dir)
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .output()
        .unwrap();
    let s = stdout(&out);
    assert!(s.contains("writing-commands"), "一覧が出るべき:\n{s}");
    assert!(
        !sb.path("fzf-was-called").exists(),
        "非TTYでfzfが呼ばれてはならない"
    );
}

#[test]
fn ツリーのdocsは同梱の同名トピックに勝つ() {
    let sb = Sandbox::new("docs-shadow");
    sb.write(
        "proj/.haj/docs/writing-commands.md",
        "# このプロジェクト流のコマンドの書き方\n",
    );

    let cp = sb.path("nonexistent");
    let out = stdout(&sb.haj(
        &sb.path("proj"),
        cp.to_str().unwrap(),
        &["docs", "writing-commands"],
    ));
    assert!(
        out.contains("このプロジェクト流"),
        "ツリー側が勝っていない:\n{out}"
    );
}

#[test]
fn 未知のトピックはエラー() {
    let sb = Sandbox::new("docs-unknown");
    let cp = sb.path("nonexistent");
    let out = sb.haj(&sb.dir, cp.to_str().unwrap(), &["docs", "no-such-topic"]);
    assert_eq!(out.status.code(), Some(1));
}

// ---- haj tree(SPEC §9.5): 共有ツリーの配布 ----

/// サンドボックス内に「配布元」の git リポジトリを作る。
fn git_remote(sb: &Sandbox, rel: &str) -> PathBuf {
    let dir = sb.path(rel);
    fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "--quiet", "-b", "main"]);
    dir
}

fn git(dir: &Path, args: &[&str]) {
    let st = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.email=t@example.com", "-c", "user.name=t"])
        .args(args)
        .output()
        .unwrap();
    assert!(
        st.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&st.stderr)
    );
}

fn commit_all(dir: &Path, msg: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "--quiet", "-m", msg]);
}

#[test]
fn ツリーをinstallすると探索に乗りupdateで差分が見えremoveで消える() {
    let sb = Sandbox::new("tree-lifecycle");
    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    // 配布元: ルート直下に commands/(形態1)
    let remote = git_remote(&sb, "remote/tools");
    sb.command(
        "remote/tools",
        "greet",
        &conforming("あいさつ", "", "", "echo HELLO"),
    );
    commit_all(&remote, "greet");

    // install(名前はリポジトリ名 tools)
    let out = sb.haj(&sb.dir, cp, &["tree", "install", remote.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "install 失敗: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // 探索に乗って実行できる。出自ラベルも出る
    let out = sb.haj(&sb.dir, cp, &["greet"]);
    assert_eq!(
        stdout(&out).trim(),
        "HELLO",
        "ツリーのコマンドが実行できない"
    );
    let help = stdout(&sb.haj(&sb.dir, cp, &["help"]));
    assert!(help.contains("[tools]"), "出自が出ない:\n{help}");

    // list に出る
    let list = stdout(&sb.haj(&sb.dir, cp, &["tree", "list"]));
    assert!(list.contains("tools"), "list に出ない:\n{list}");

    // 配布元にコマンドを足して update → 差分が見えて、新コマンドが使える
    sb.command(
        "remote/tools",
        "bye",
        &conforming("わかれ", "", "", "echo BYE"),
    );
    commit_all(&remote, "byeを追加");
    let out = sb.haj(&sb.dir, cp, &["tree", "update"]);
    assert!(
        out.status.success(),
        "update 失敗: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout(&out).contains("byeを追加"),
        "差分が見えない:\n{}",
        stdout(&out)
    );
    let out = sb.haj(&sb.dir, cp, &["bye"]);
    assert_eq!(stdout(&out).trim(), "BYE");

    // 最新なら「最新です」
    let out = sb.haj(&sb.dir, cp, &["tree", "update", "tools"]);
    assert!(stdout(&out).contains("最新"), "{}", stdout(&out));

    // remove で探索から消える
    let out = sb.haj(&sb.dir, cp, &["tree", "remove", "tools"]);
    assert!(out.status.success());
    let out = sb.haj(&sb.dir, cp, &["greet"]);
    assert_eq!(out.status.code(), Some(127), "remove 後も残っている");
}

#[test]
fn haj形式のリポジトリとconfigの名前とエイリアス配布() {
    let sb = Sandbox::new("tree-dothaj");
    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    // 形態2: .haj/ を持つ普通のプロジェクト。config で名前とエイリアスを配る
    let remote = git_remote(&sb, "remote/myapp");
    sb.command(
        "remote/myapp/.haj",
        "deploy",
        &conforming("デプロイ", "", "", "echo DEPLOYED"),
    );
    sb.write(
        "remote/myapp/.haj/config",
        "name = shared-tools\nalias.hi = sh -- echo HI_FROM_TREE\nalias.hi.desc = ツリー配布のエイリアス\n",
    );
    commit_all(&remote, "tree");

    let out = sb.haj(&sb.dir, cp, &["tree", "install", remote.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "install 失敗: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // 名前は config の name が勝つ
    assert!(
        stdout(&out).contains("shared-tools"),
        "config の name が使われない:\n{}",
        stdout(&out)
    );

    // .haj の下の commands が探索に乗る
    let out = sb.haj(&sb.dir, cp, &["deploy"]);
    assert_eq!(stdout(&out).trim(), "DEPLOYED");

    // ツリー config のエイリアスが効く(スコープは ユーザー設定より遠い)
    let out = sb.haj(&sb.dir, cp, &["hi"]);
    assert_eq!(
        stdout(&out).trim(),
        "HI_FROM_TREE",
        "ツリーのエイリアスが効かない"
    );
    let help = stdout(&sb.haj(&sb.dir, cp, &["help"]));
    assert!(
        help.contains("ツリー配布のエイリアス"),
        "desc が出ない:\n{help}"
    );
}

#[test]
fn グローバルにもインストールできる() {
    let sb = Sandbox::new("tree-global");
    let cp = sb.path("nonexistent");

    let remote = git_remote(&sb, "remote/shared");
    sb.command(
        "remote/shared",
        "ping",
        &conforming("疎通", "", "", "echo PONG"),
    );
    commit_all(&remote, "ping");

    // XDG_DATA_DIRS をサンドボックスに向けて --global で入れる
    let run = |args: &[&str]| -> Output {
        Command::new(env!("CARGO_BIN_EXE_haj"))
            .args(args)
            .current_dir(&sb.dir)
            .env("HAJ_COMMAND_PATH", &cp)
            .env("HAJ_NO_CACHE", "1")
            .env("HOME", &sb.dir)
            .env("XDG_DATA_DIRS", sb.path("xdgdata"))
            .output()
            .unwrap()
    };
    let out = run(&["tree", "install", "--global", remote.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "global install 失敗: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        sb.path("xdgdata/haj/trees/shared").is_dir(),
        "XDG_DATA_DIRS 先に入っていない"
    );

    // 探索に乗る(個人の trees には入っていない)
    let out = run(&["ping"]);
    assert_eq!(
        stdout(&out).trim(),
        "PONG",
        "グローバルツリーが探索されない"
    );
    let list = stdout(&run(&["tree", "list"]));
    assert!(list.contains("shared"), "list に出ない:\n{list}");
}

#[test]
fn 空のリポジトリはツリーとして認めない() {
    let sb = Sandbox::new("tree-reject");
    let cp = sb.path("nonexistent");

    let remote = git_remote(&sb, "remote/junk");
    sb.write("remote/junk/README.md", "ただのリポジトリ\n");
    commit_all(&remote, "junk");

    let out = sb.haj(
        &sb.dir,
        cp.to_str().unwrap(),
        &["tree", "install", remote.to_str().unwrap()],
    );
    assert_eq!(out.status.code(), Some(1), "ゴミが入ってしまう");
    // 失敗したものが置き場に残らない
    let trees = sb.path(".local/share/haj/trees");
    let leftover = fs::read_dir(&trees).map(|d| d.count()).unwrap_or(0);
    assert_eq!(leftover, 0, "失敗の残骸がある");
}

// ---- エイリアス(SPEC §2.7): git 方式 ----

/// XDG_CONFIG_HOME を差し替えて haj を走らせる(エイリアスのテスト用)。
fn haj_with_config(sb: &Sandbox, cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_haj"))
        .args(args)
        .current_dir(cwd)
        .env("HAJ_COMMAND_PATH", sb.path("nonexistent"))
        .env("HAJ_NO_CACHE", "1")
        .env("HOME", &sb.dir)
        .env("XDG_CONFIG_HOME", sb.path("xdgconf"))
        .output()
        .unwrap()
}

#[test]
fn エイリアスは名前の位置で展開され残りの引数が続く() {
    let sb = Sandbox::new("alias");
    sb.write("xdgconf/haj/config", "alias.pj = -C proj\n");
    sb.command(
        "proj/.haj",
        "deploy",
        &conforming("デプロイ", "", "", "echo DEPLOYED $1"),
    );

    let out = haj_with_config(&sb, &sb.dir, &["pj", "deploy", "v2"]);
    assert!(out.status.success(), "エイリアス越しに実行できない");
    assert_eq!(stdout(&out).trim(), "DEPLOYED v2");
}

#[test]
fn エイリアスは予約語を奪えない() {
    let sb = Sandbox::new("alias-reserved");
    sb.write("xdgconf/haj/config", "alias.help = -C proj\n");

    let out = haj_with_config(&sb, &sb.dir, &["help"]);
    assert!(out.status.success());
    // 普通のヘルプが出る(展開されて proj へ行ったりしない)
    assert!(stdout(&out).contains("haj自身"), "helpが奪われた");
}

#[test]
fn whichとhelpと補完にエイリアスの素性が出る() {
    let sb = Sandbox::new("alias-vis");
    sb.write("xdgconf/haj/config", "alias.ie = -C ~/repos/ie\n");

    let which = haj_with_config(&sb, &sb.dir, &["which", "ie"]);
    assert!(which.status.success());
    assert!(
        stdout(&which).contains("alias.ie = -C ~/repos/ie"),
        "whichが展開を見せない:\n{}",
        stdout(&which)
    );

    let help = stdout(&haj_with_config(&sb, &sb.dir, &["help"]));
    assert!(help.contains("エイリアス"), "helpに節が無い:\n{help}");
    assert!(help.contains("ie"), "helpに名前が無い:\n{help}");

    let comp = stdout(&haj_with_config(&sb, &sb.dir, &["__complete"]));
    assert!(comp.contains("ie\t"), "補完に出ない:\n{comp}");
}

#[test]
fn プロジェクトのhaj_projectにエイリアスを書ける() {
    let sb = Sandbox::new("alias-proj");
    sb.command(
        "proj/.haj",
        "deploy",
        &conforming("デプロイ", "", "", "echo DEPLOYED $1"),
    );
    sb.write(
        "proj/.haj/config",
        "name = myapp\nalias.t = deploy v9\nalias.t.desc = テストを流す\n",
    );

    // プロジェクトの中では効く(残りの引数は後ろに繋がらない形で固定値を検証)
    let out = haj_with_config(&sb, &sb.path("proj"), &["t"]);
    assert!(
        out.status.success(),
        "プロジェクト・エイリアスで実行できない"
    );
    assert_eq!(stdout(&out).trim(), "DEPLOYED v9");

    // help に .desc と出自(プロジェクト名)が出る
    let help = stdout(&haj_with_config(&sb, &sb.path("proj"), &["help"]));
    assert!(help.contains("テストを流す"), "descが出ない:\n{help}");
    assert!(help.contains("[myapp]"), "出自が出ない:\n{help}");

    // which も展開と出自を見せる
    let which = stdout(&haj_with_config(&sb, &sb.path("proj"), &["which", "t"]));
    assert!(
        which.contains("alias.t = deploy v9"),
        "whichが展開を見せない:\n{which}"
    );
    assert!(which.contains("[myapp]"), "whichが出自を見せない:\n{which}");

    // 補完にも出る
    let comp = stdout(&haj_with_config(&sb, &sb.path("proj"), &["__complete"]));
    assert!(comp.contains("t\tテストを流す"), "補完に出ない:\n{comp}");

    // プロジェクトの外では存在しない
    let out = haj_with_config(&sb, &sb.dir, &["t"]);
    assert!(!out.status.success(), "プロジェクトの外で効いてしまう");
}

#[test]
fn プロジェクトのエイリアスはユーザー設定より勝つ() {
    let sb = Sandbox::new("alias-scope");
    sb.write("xdgconf/haj/config", "alias.t = deploy GLOBAL\n");
    sb.command(
        "proj/.haj",
        "deploy",
        &conforming("デプロイ", "", "", "echo DEPLOYED $1"),
    );
    sb.write("proj/.haj/config", "alias.t = deploy PROJECT\n");

    let out = haj_with_config(&sb, &sb.path("proj"), &["t"]);
    assert_eq!(
        stdout(&out).trim(),
        "DEPLOYED PROJECT",
        "近いスコープが勝っていない"
    );
}

#[test]
fn プロジェクトエイリアスでも予約語は奪えない() {
    let sb = Sandbox::new("alias-proj-reserved");
    sb.write("proj/.haj/config", "alias.help = sh 'echo HIJACKED'\n");
    std::fs::create_dir_all(sb.path("proj/.haj")).unwrap();

    let out = haj_with_config(&sb, &sb.path("proj"), &["help"]);
    assert!(out.status.success());
    let s = stdout(&out);
    assert!(!s.contains("HIJACKED"), "helpが奪われた:\n{s}");
    assert!(s.contains("haj自身"), "普通のhelpが出ていない:\n{s}");
}

#[test]
fn プロジェクトのconfigから接続先の鍵は読まれない() {
    // clone したリポジトリに secrets.* / selfupgrade.* / command_path を書かれても
    // 効かない(ホワイトリスト: name / root / alias.* だけ)。SPEC §2.2。
    let sb = Sandbox::new("proj-config-whitelist");
    sb.write(
        "proj/.haj/config",
        "secrets.vault_addr = https://evil.example\ncommand_path = /evil\n",
    );

    let out = haj_with_config(&sb, &sb.path("proj"), &["config"]);
    assert!(out.status.success());
    let s = stdout(&out);
    assert!(
        !s.contains("evil"),
        "プロジェクト config が実効値に混ざった:\n{s}"
    );
}

#[test]
fn 旧home_haj_commandsはもう読まれない() {
    let sb = Sandbox::new("no-legacy");
    // HOME = sb.dir なので、これは ~/.haj/commands/old
    sb.command(".haj", "old", &conforming("旧置き場", "", "", "echo OLD"));

    let cp = sb.path("nonexistent");
    let out = sb.haj(&sb.dir, cp.to_str().unwrap(), &["old"]);
    assert_eq!(
        out.status.code(),
        Some(127),
        "旧 ~/.haj/commands が読まれている"
    );
}

#[test]
fn execとshにもhaj_projectが渡る() {
    let sb = Sandbox::new("sh-project");
    sb.write("proj/.haj/config", "name = myapp\n");

    let cp = sb.path("nonexistent");
    let out = sb.haj(
        &sb.path("proj"),
        cp.to_str().unwrap(),
        &["sh", "--", "echo", "P=$HAJ_PROJECT"],
    );
    assert_eq!(
        stdout(&out).trim(),
        "P=myapp",
        "sh に HAJ_PROJECT が渡らない"
    );

    // プロジェクトの外では空(呼び出し元の環境の値も継がない)
    let out = sb.haj(
        &sb.dir,
        cp.to_str().unwrap(),
        &["sh", "--", "echo", "P=$HAJ_PROJECT"],
    );
    assert_eq!(
        stdout(&out).trim(),
        "P=",
        "プロジェクト外で HAJ_PROJECT が残っている"
    );
}

#[test]
fn ハイフンcのチルダはhomeに展開される() {
    let sb = Sandbox::new("chdir-tilde");
    sb.command("sub/.haj", "inner", &conforming("内側", "", "", "true"));

    // HOME = sb.dir なので ~/sub は sb.dir/sub
    let out = haj_with_config(&sb, &sb.dir, &["-C", "~/sub", "inner"]);
    assert!(out.status.success(), "チルダが展開されていない");
}

// ---- haj completion(SPEC §9.4)----

#[test]
fn completionは補完スクリプトを出す() {
    let sb = Sandbox::new("completion");
    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    let zsh = sb.haj(&sb.dir, cp, &["completion", "zsh"]);
    assert!(zsh.status.success());
    let s = stdout(&zsh);
    assert!(s.starts_with("#compdef haj"), "zsh補完ではない:\n{s}");
    assert!(s.contains("__complete"), "コアに聞く形になっていない");

    let bash = stdout(&sb.haj(&sb.dir, cp, &["completion", "bash"]));
    assert!(bash.contains("complete -F"), "bash補完ではない:\n{bash}");

    // 未対応シェルと引数なしは使い方エラー
    assert_eq!(
        sb.haj(&sb.dir, cp, &["completion", "fish"]).status.code(),
        Some(1)
    );
    assert_eq!(sb.haj(&sb.dir, cp, &["completion"]).status.code(), Some(1));
}

#[test]
fn 設定は行末のバックスラッシュで継続できる() {
    let sb = Sandbox::new("cfg-cont");
    // 長い alias は1行に収まらない。継続行で書けること。
    sb.write(
        "xdgconf/haj/config",
        "alias.pj = -C proj \\\n           --secret HAJ_T_VALUE=hello \\\n           deploy\n",
    );
    sb.command(
        "proj/.haj",
        "deploy",
        "#!/bin/sh\ncase \"$1\" in --haj-*) exit 0 ;; esac\nprintf '%s\\n' \"$HAJ_T_VALUE\"\n",
    );

    let out = Command::new(env!("CARGO_BIN_EXE_haj"))
        .args(["pj"])
        .current_dir(&sb.dir)
        .env("HAJ_COMMAND_PATH", sb.path("nonexistent"))
        .env("HAJ_NO_CACHE", "1")
        .env("HOME", &sb.dir)
        .env("XDG_CONFIG_HOME", sb.path("xdgconf"))
        .output()
        .unwrap();

    assert!(out.status.success(), "継続行が繋がっていない");
    assert_eq!(stdout(&out).trim(), "hello");
}

#[test]
fn エイリアスの補完は展開してから転送する() {
    let sb = Sandbox::new("alias-comp");
    sb.write("xdgconf/haj/config", "alias.pj = -C proj\n");
    sb.command(
        "proj/.haj",
        "mig",
        &conforming("マイグレーション", "", "up down", "true"),
    );

    let out = Command::new(env!("CARGO_BIN_EXE_haj"))
        .args(["__complete", "pj"])
        .current_dir(&sb.dir)
        .env("HAJ_COMMAND_PATH", sb.path("nonexistent"))
        .env("HAJ_NO_CACHE", "1")
        .env("HOME", &sb.dir)
        .env("XDG_CONFIG_HOME", sb.path("xdgconf"))
        .output()
        .unwrap();

    // -C proj で移動した先のコマンドが候補に出る
    assert!(
        stdout(&out).contains("mig"),
        "エイリアス越しに補完できていない:\n{}",
        stdout(&out)
    );
}

#[test]
fn execの補完はそのコマンド自身へ委譲する() {
    let sb = Sandbox::new("exec-comp");
    sb.write(
        "xdgconf/haj/config",
        "alias.oci = --secret K=v exec oci\nalias.oci.desc = OCI CLI を起動する\n",
    );

    let haj = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_haj"))
            .args(args)
            .current_dir(&sb.dir)
            .env("HAJ_COMMAND_PATH", sb.path("nonexistent"))
            .env("HAJ_NO_CACHE", "1")
            .env("HOME", &sb.dir)
            .env("XDG_CONFIG_HOME", sb.path("xdgconf"))
            .output()
            .unwrap()
    };

    // エイリアス経由: グローバルフラグを読み飛ばし、exec の先へ委譲する
    let out = stdout(&haj(&["__complete", "oci", "iam"]));
    assert_eq!(out.trim(), "@delegate\toci\tiam", "委譲の指示が違う: {out}");

    // 直接の exec でも同じ
    let out = stdout(&haj(&["__complete", "exec", "kubectl", "get"]));
    assert_eq!(out.trim(), "@delegate\tkubectl\tget");

    // 説明は alias.<名前>.desc が使われる(長い展開の代わりに)
    let list = stdout(&haj(&["__complete"]));
    assert!(
        list.contains("oci\tOCI CLI を起動する"),
        "descが補完の説明に出ていない:\n{list}"
    );
    // .desc 自体はエイリアスとして現れない
    assert!(!list.contains("oci.desc"), "descがコマンド名になっている");

    // help の一覧にも desc が出る
    let help = stdout(&haj(&["help"]));
    assert!(
        help.contains("OCI CLI を起動する"),
        "helpにdescが無い:\n{help}"
    );
}

#[test]
fn completionのzsh版はevalしても補完関数を即実行しない() {
    let sb = Sandbox::new("comp-eval");
    let cp = sb.path("nonexistent");
    let out = sb.haj(&sb.dir, cp.to_str().unwrap(), &["completion", "zsh"]);
    let s = stdout(&out);

    // eval で読み込まれたときは compdef で登録するだけにする。
    // 補完コンテキストの外で _haj を呼ぶと _describe が
    // "can only be called from completion function" で怒る。
    assert!(s.contains("compdef _haj haj"), "eval 用の登録が無い:\n{s}");
    assert!(
        s.contains("funcstack[1]"),
        "autoload と eval を見分けていない:\n{s}"
    );
    // 委譲時、カーソル位置の語は引用符で保つこと。空語が配列から落ちると
    // CURRENT が1になり、_normal が PATH 上の全コマンドを出してしまう。
    assert!(
        s.contains("\"${words[CURRENT]}\""),
        "カーソル位置の語が引用されていない:\n{s}"
    );

    // 即時呼び出しはガードの中だけ(スクリプトの最後は fi で閉じている)
    let last = s.lines().rfind(|l| !l.trim().is_empty()).unwrap();
    assert_eq!(
        last.trim(),
        "fi",
        "即時呼び出しがガードの外にある(末尾: {last})"
    );
}
