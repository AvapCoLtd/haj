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
            .env("HOME", &self.dir) // ~/.haj を汚さない
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
    sb.write("mono/web/.haj/project", "name = web\nroot = false\n");

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
    sb.write("proj/.haj/project", "name = example-app\n");

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
    // .haj/project を置いていない

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
// 形式は .haj/project と同じ key = value(覚えることを1つに保つ)。

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
    sb.write("proj/.haj/project", "name = example-app\n");

    let cp = sb.path("none");
    let out = stdout(&sb.haj(&sb.path("proj"), cp.to_str().unwrap(), &["setup"]));

    assert_eq!(
        out.trim(),
        format!(
            "project=example-app dir={}",
            sb.path("proj").display()
        ),
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
    sb.write("proj/.haj/project", "name = myproj\n");

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
    for f in ["-C ", "--secret ", "--env ", "--secretfile "] {
        assert!(s.contains(f), "{f} がヘルプに無い:\n{s}");
    }
}
