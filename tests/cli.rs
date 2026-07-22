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
            .env_remove("XDG_CONFIG_HOME") // ホストの XDG 設定に依存しない
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
fn 補完はグローバルフラグを読み飛ばす() {
    // `haj --env-file f <TAB>` でコマンド名が、`haj --env-file f mig <TAB>` で
    // mig の候補が出る(SPEC §6)。シェル側は語を素通しで渡すだけ。
    let sb = Sandbox::new("complete-flags");
    sb.command(
        "sys",
        "mig",
        r#"#!/bin/sh
case "$1" in
  --haj-describe) echo "マイグレーション"; exit 0 ;;
  --haj-complete) shift; if [ $# -eq 0 ]; then printf '%s\n' up down; fi; exit 0 ;;
esac
"#,
    );
    sb.command(
        "proj/.haj",
        "deploy",
        &conforming("このリポジトリ専用", "", "", "true"),
    );
    sb.write("dummy.env", "A=b\n");

    let cp = sb.path("sys/commands");
    let cp = cp.to_str().unwrap();

    // フラグの後ろはコマンド名の位置
    let names = stdout(&sb.haj(&sb.dir, cp, &["__complete", "--env-file", "dummy.env"]));
    assert!(
        names.contains("mig"),
        "フラグの後ろで名前が出ない:\n{names}"
    );

    // フラグ込みでもサブコマンドの --haj-complete へ届く
    let ops = stdout(&sb.haj(
        &sb.dir,
        cp,
        &["__complete", "--env-file", "dummy.env", "mig"],
    ));
    assert_eq!(ops.split_whitespace().collect::<Vec<_>>(), ["up", "down"]);

    // -C は適用される(移動先のプロジェクトのコマンドが見える)
    let inside = stdout(&sb.haj(&sb.dir, cp, &["__complete", "-C", "proj"]));
    assert!(
        inside.contains("deploy"),
        "-C が反映されていない:\n{inside}"
    );

    // 値が未入力のフラグで終わる → 候補なし(値の補完はシェル側の仕事)
    let none = stdout(&sb.haj(&sb.dir, cp, &["__complete", "--env-file"]));
    assert!(
        none.trim().is_empty(),
        "値の位置で候補を出してはならない:\n{none}"
    );
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
fn 環境変数はコマンド自身に聞きenv_fileへ往復できる() {
    // haj env <名前> は --haj-env の中継(SPEC §4.4)。出力はそのまま
    // --env-file に渡せる形式なので、雛形→編集→注入、が往復する。
    let sb = Sandbox::new("env-hook");
    sb.command(
        "sys",
        "met",
        r##"#!/bin/sh
case "$1" in
  --haj-describe) echo "計測"; exit 0 ;;
  --haj-env) printf '%s\n' "# 対象DB" "FOO=${FOO:-default1}" "BAR=b"; exit 0 ;;
esac
echo "FOO=$FOO"
"##,
    );
    // --haj-env に応答しない素朴なコマンド
    sb.command("sys", "plain", "#!/bin/sh\necho ran\n");

    let cp = sb.path("sys/commands");
    let cp = cp.to_str().unwrap();

    let out = sb.haj(&sb.dir, cp, &["env", "met"]);
    assert!(out.status.success());
    let s = stdout(&out);
    assert!(s.contains("FOO=default1"), "既定値が出ない:\n{s}");
    assert!(s.contains("# 対象DB"), "コメントが落ちている:\n{s}");

    // 雛形→編集→--env-file で注入、の往復
    sb.write("env.txt", "# 対象DB\nFOO=edited\n");
    let run = stdout(&sb.haj(&sb.dir, cp, &["--env-file", "env.txt", "met"]));
    assert_eq!(run.trim(), "FOO=edited", "編集した値が渡っていない");

    // 未対応のコマンドはエラー(黙って空を出さない)
    let no = sb.haj(&sb.dir, cp, &["env", "plain"]);
    assert!(!no.status.success(), "--haj-env未対応はエラーにすべき");
}

#[test]
fn configの雛形にdocsの鍵が載る() {
    // 設定できる鍵は haj config --init が**すべて**雛形として出す(SPEC §8.2)。
    // docs.* を KEYS に足し忘れると、設定できるのに発見できない鍵になる。
    let sb = Sandbox::new("config-docs-keys");
    let cp = sb.path("nonexistent");
    let s = stdout(&sb.haj(&sb.dir, cp.to_str().unwrap(), &["config", "--init"]));
    for k in [
        "docs.fzf_cmd",
        "docs.fzf_args",
        "docs.preview_cmd",
        "docs.pager",
    ] {
        assert!(s.contains(k), "{k} が雛形に無い:\n{s}");
    }
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

    // ファイル補完の指示(SPEC §4.3)を両シェルのスクリプトが処理する
    assert!(
        s.contains("@files") && s.contains("_files") && s.contains("_directories"),
        "zsh補完が @files / @dirs を処理していない:\n{s}"
    );
    assert!(
        bash.contains("@files") && bash.contains("compgen -d"),
        "bash補完が @files / @dirs を処理していない:\n{bash}"
    );

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
fn ファイル補完の指示はコアが素通しで届ける() {
    // @files / @dirs(SPEC §4.3)はシェルへの指示。コアは解釈せず、
    // --haj-complete の出力をそのままシェル補完スクリプトへ渡す(SPEC §6)。
    let sb = Sandbox::new("files-directive");
    sb.command(
        "sys",
        "transcode",
        r#"#!/bin/sh
case "$1" in
  --haj-describe) echo "動画を変換する"; exit 0 ;;
  --haj-complete)
    shift
    case $# in
      0) printf '@files\t*.mp4\t*.mkv\n--force\n' ;;  # 1語目: 入力 + 通常の候補
      1) echo '@dirs' ;;                              # 2語目: 出力先
    esac
    exit 0 ;;
esac
"#,
    );

    let cp = sb.path("sys/commands");
    let cp = cp.to_str().unwrap();

    let first = stdout(&sb.haj(&sb.dir, cp, &["__complete", "transcode"]));
    assert_eq!(
        first.lines().collect::<Vec<_>>(),
        ["@files\t*.mp4\t*.mkv", "--force"],
        "指示が素通しになっていない"
    );

    let second = stdout(&sb.haj(&sb.dir, cp, &["__complete", "transcode", "in.mp4"]));
    assert_eq!(second.trim(), "@dirs");
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

// ---- タスク(SPEC §9.6): haj run ----

/// `.haj/tasks/<名前>` に実行可能なタスクを置く。
fn task_file(sb: &Sandbox, project: &str, name: &str, body: &str) -> PathBuf {
    let dir = sb.dir.join(project).join(".haj").join("tasks");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn タスクはrunで実行され素性の環境変数が渡る() {
    let sb = Sandbox::new("task-run");
    fs::create_dir_all(sb.path("proj/.haj")).unwrap();
    task_file(
        &sb,
        "proj",
        "build",
        "#!/bin/sh\necho BUILT $1 name=$HAJ_NAME project=$HAJ_PROJECT root=$HAJ_ROOT\n",
    );

    let out = haj_with_config(&sb, &sb.path("proj"), &["run", "build", "v2"]);
    assert!(
        out.status.success(),
        "タスクを実行できない: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // HAJ_NAME はタスクの名前(run ではない)。HAJ_ROOT はプロジェクトの .haj。
    assert_eq!(
        stdout(&out).trim(),
        format!(
            "BUILT v2 name=build project=proj root={}",
            sb.path("proj/.haj").display()
        )
    );
}

#[test]
fn タスクとコマンドの名前空間は交わらない() {
    let sb = Sandbox::new("task-ns");
    task_file(&sb, "proj", "install", "#!/bin/sh\necho TASK\n");
    sb.command("proj/.haj", "deploy", "#!/bin/sh\necho CMD\n");

    // タスクは素の名前で呼べない(探索に乗らない)
    let out = haj_with_config(&sb, &sb.path("proj"), &["install"]);
    assert_eq!(out.status.code(), Some(127), "タスクが素の名前で生えている");

    // コマンドは run で呼べない(フォールバックしない)。代わりに案内を出す
    let out = haj_with_config(&sb, &sb.path("proj"), &["run", "deploy"]);
    assert_eq!(out.status.code(), Some(127));
    let err = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        err.contains("コマンドとして定義"),
        "コマンドへの案内が出ない:\n{err}"
    );
}

#[test]
fn タスクの1行宣言は展開されファイルより勝つ() {
    let sb = Sandbox::new("task-decl");
    sb.write(
        "proj/.haj/config",
        "name = myapp\ntask.hi = sh -- echo DECL $HAJ_PROJECT\ntask.hi.desc = あいさつ\n",
    );
    task_file(&sb, "proj", "hi", "#!/bin/sh\necho FILE\n");

    let out = haj_with_config(&sb, &sb.path("proj"), &["run", "hi"]);
    assert!(
        out.status.success(),
        "宣言タスクを実行できない: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(stdout(&out).trim(), "DECL myapp", "宣言が勝っていない");
}

#[test]
fn runはプロジェクトの外では使えず親へも遡らない() {
    let sb = Sandbox::new("task-boundary");

    // プロジェクトの外は exit 1
    let out = haj_with_config(&sb, &sb.dir, &["run", "x"]);
    assert_eq!(out.status.code(), Some(1), "プロジェクトの外で run が通る");

    // 親プロジェクトのタスクは、root = false の子からも見えない(遡らない)
    task_file(&sb, "parent", "x", "#!/bin/sh\necho PARENT\n");
    sb.write("parent/child/.haj/config", "root = false\n");
    let out = haj_with_config(&sb, &sb.path("parent/child"), &["run", "x"]);
    assert_eq!(out.status.code(), Some(127), "親のタスクへ遡っている");

    // 親のプロジェクトの中では動く
    let out = haj_with_config(&sb, &sb.path("parent"), &["run", "x"]);
    assert_eq!(stdout(&out).trim(), "PARENT");
}

#[test]
fn run一覧とhelpの節と補完にタスクが出る() {
    let sb = Sandbox::new("task-list");
    sb.write(
        "proj/.haj/config",
        "name = myapp\ntask.up = sh -- echo UP\ntask.up.desc = 上げる\n",
    );
    task_file(
        &sb,
        "proj",
        "check",
        &conforming("検査する", "検査の使い方", "quick full", "echo CHECKED"),
    );

    // haj run(引数なし)= 一覧。宣言もファイルも説明つきで出る
    let list = stdout(&haj_with_config(&sb, &sb.path("proj"), &["run"]));
    assert!(
        list.contains("up") && list.contains("上げる"),
        "一覧に宣言が出ない:\n{list}"
    );
    assert!(
        list.contains("check") && list.contains("検査する"),
        "一覧にファイルが出ない:\n{list}"
    );

    // help にタスクの節が出る
    let help = stdout(&haj_with_config(&sb, &sb.path("proj"), &["help"]));
    assert!(
        help.contains("プロジェクトのタスク"),
        "helpに節が無い:\n{help}"
    );
    assert!(help.contains("check"), "helpにタスク名が無い:\n{help}");

    // __complete run → "名前\t説明" のタスク一覧
    let comp = stdout(&haj_with_config(
        &sb,
        &sb.path("proj"),
        &["__complete", "run"],
    ));
    assert!(comp.contains("check\t検査する"), "補完に出ない:\n{comp}");
    assert!(comp.contains("up\t上げる"), "宣言が補完に出ない:\n{comp}");

    // __complete run <名前> → そのタスクの --haj-complete へ転送
    let comp = stdout(&haj_with_config(
        &sb,
        &sb.path("proj"),
        &["__complete", "run", "check"],
    ));
    assert!(
        comp.contains("quick"),
        "タスクの補完が転送されない:\n{comp}"
    );

    // タスクは素の名前の補完(コマンド一覧)には出ない
    let comp = stdout(&haj_with_config(&sb, &sb.path("proj"), &["__complete"]));
    assert!(
        !comp.contains("check\t") && !comp.contains("up\t"),
        "素の一覧にタスクが漏れている:\n{comp}"
    );
}

#[test]
fn helpとenvとwhichはrun合成形でタスクに答える() {
    let sb = Sandbox::new("task-meta");
    sb.write("proj/.haj/config", "task.hi = sh -- echo HI\n");
    task_file(
        &sb,
        "proj",
        "check",
        &conforming("検査する", "検査の使い方", "", "echo CHECKED"),
    );

    let help = stdout(&haj_with_config(
        &sb,
        &sb.path("proj"),
        &["help", "run", "check"],
    ));
    assert!(
        help.contains("検査の使い方"),
        "--haj-help が出ない:\n{help}"
    );

    // which run は効いている定義(宣言の展開 / ファイルのパス)を見せる
    let which = stdout(&haj_with_config(
        &sb,
        &sb.path("proj"),
        &["which", "run", "hi"],
    ));
    assert!(
        which.contains("task.hi = sh -- echo HI"),
        "whichが宣言を見せない:\n{which}"
    );
    let which = stdout(&haj_with_config(
        &sb,
        &sb.path("proj"),
        &["which", "run", "check"],
    ));
    assert!(
        which.contains(&sb.path("proj/.haj/tasks/check").display().to_string()),
        "whichがパスを見せない:\n{which}"
    );

    // env run は --haj-env の中継(コマンドと同じ形の検証つき)
    let envtask =
        "#!/bin/sh\ncase \"$1\" in --haj-env) echo 'CHECK_LEVEL=1'; exit 0 ;; esac\necho RUN\n";
    task_file(&sb, "proj", "metrics", envtask);
    let env = stdout(&haj_with_config(
        &sb,
        &sb.path("proj"),
        &["env", "run", "metrics"],
    ));
    assert!(
        env.contains("CHECK_LEVEL=1"),
        "--haj-env が中継されない:\n{env}"
    );
}

#[test]
fn runは予約語だが名前空間の中では予約語の名前も使える() {
    let sb = Sandbox::new("task-reserved");
    // .haj/commands/run を置いても組み込みの run は奪えない
    sb.command("proj/.haj", "run", "#!/bin/sh\necho HIJACKED\n");
    task_file(&sb, "proj", "help", "#!/bin/sh\necho TASKHELP\n");
    task_file(&sb, "proj", "ok", "#!/bin/sh\necho OK\n");

    let out = haj_with_config(&sb, &sb.path("proj"), &["run", "ok"]);
    assert_eq!(stdout(&out).trim(), "OK", "組み込みの run が奪われた");

    // 名前空間の中に組み込みは居ないので、予約語の名前のタスクも呼べる。
    // 素の haj help は従来どおり組み込みのまま (予約語が守るのは素の名前だけ)
    let out = haj_with_config(&sb, &sb.path("proj"), &["run", "help"]);
    assert_eq!(
        stdout(&out).trim(),
        "TASKHELP",
        "run 名前空間で予約語名が使えない"
    );
    let out = haj_with_config(&sb, &sb.path("proj"), &["help"]);
    assert!(
        stdout(&out).contains("haj自身"),
        "素の help が奪われた:\n{}",
        stdout(&out)
    );
}

#[test]
fn ユーザー設定のtask鍵は無視される() {
    // タスクはプロジェクト局所の概念。task.* はプロジェクトの .haj/config からしか
    // 読まない(どこでも効かせたいものはエイリアスかコマンドにする)。
    let sb = Sandbox::new("task-scope");
    sb.write("xdgconf/haj/config", "task.hi = sh -- echo USER\n");
    fs::create_dir_all(sb.path("proj/.haj")).unwrap();

    let out = haj_with_config(&sb, &sb.path("proj"), &["run", "hi"]);
    assert_eq!(
        out.status.code(),
        Some(127),
        "ユーザー設定の task.* が効いている"
    );
}

#[test]
fn エイリアスからrunへ委譲でき宣言の展開は1回だけ() {
    let sb = Sandbox::new("task-chain");
    sb.write("xdgconf/haj/config", "alias.t = run hi\n");
    sb.write(
        "proj/.haj/config",
        "task.hi = sh -- echo HI\ntask.a = run b\ntask.b = sh -- echo B\ntask.k = exec git\n",
    );

    // エイリアス → run 宣言 は動く(エイリアス1回 + タスク1回。フラグは別)
    let out = haj_with_config(&sb, &sb.path("proj"), &["t"]);
    assert_eq!(
        stdout(&out).trim(),
        "HI",
        "エイリアス経由で宣言タスクが動かない: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // task.a = run b(宣言 → 宣言)は再展開しない(エイリアスの「再帰しない」と同じ)
    let out = haj_with_config(&sb, &sb.path("proj"), &["run", "a"]);
    assert_eq!(out.status.code(), Some(127), "宣言が再帰展開されている");

    // 宣言が exec に解決されたら、補完はそのコマンド自身へ委譲する(@delegate)
    let comp = stdout(&haj_with_config(
        &sb,
        &sb.path("proj"),
        &["__complete", "run", "k"],
    ));
    assert!(
        comp.starts_with("@delegate\tgit"),
        "execへの委譲指示が出ない:\n{comp}"
    );
}

// ---- ツリー名前空間(SPEC §9.7): haj <ツリー名> <名前> ----

#[test]
fn ツリー名前空間は常に使えexposeで素の露出が消える() {
    let sb = Sandbox::new("tree-ns");
    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    // flat(既定)のツリー: 素の形も名前空間も両方使える
    let remote = git_remote(&sb, "remote/tools");
    sb.command(
        "remote/tools",
        "greet",
        &conforming("あいさつ", "あいさつの使い方", "loud quiet", "echo HELLO"),
    );
    commit_all(&remote, "greet");
    let out = sb.haj(&sb.dir, cp, &["tree", "install", remote.to_str().unwrap()]);
    assert!(out.status.success());

    let out = sb.haj(&sb.dir, cp, &["greet"]);
    assert_eq!(stdout(&out).trim(), "HELLO", "flat の素の形が使えない");
    let out = sb.haj(&sb.dir, cp, &["tools", "greet"]);
    assert_eq!(stdout(&out).trim(), "HELLO", "名前空間の明示形が使えない");

    // expose = namespace のツリー: 素の形から消え、名前空間でだけ呼べる
    let remote2 = git_remote(&sb, "remote/ext");
    sb.command(
        "remote/ext",
        "install",
        &conforming("入れる", "入れ方", "", "echo INSTALLED $1"),
    );
    sb.write("remote/ext/config", "name = ext\nexpose = namespace\n");
    commit_all(&remote2, "ext");
    let out = sb.haj(&sb.dir, cp, &["tree", "install", remote2.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "install 失敗: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = sb.haj(&sb.dir, cp, &["install"]);
    assert_eq!(
        out.status.code(),
        Some(127),
        "namespace ツリーのコマンドが素の形で生えている"
    );
    let out = sb.haj(&sb.dir, cp, &["ext", "install", "foo"]);
    assert_eq!(
        stdout(&out).trim(),
        "INSTALLED foo",
        "名前空間で実行できない: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // haj <ツリー名>(引数なし)は一覧
    let list = stdout(&sb.haj(&sb.dir, cp, &["ext"]));
    assert!(
        list.contains("install") && list.contains("入れる"),
        "一覧が出ない:\n{list}"
    );

    // 未知の名前は 127
    let out = sb.haj(&sb.dir, cp, &["ext", "nope"]);
    assert_eq!(out.status.code(), Some(127));

    // tree list に (namespace) と出る
    let list = stdout(&sb.haj(&sb.dir, cp, &["tree", "list"]));
    assert!(list.contains("(namespace)"), "list に出ない:\n{list}");
}

#[test]
fn ツリー名前空間の合成形と補完とhelpの入口() {
    let sb = Sandbox::new("tree-ns-meta");
    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    let remote = git_remote(&sb, "remote/ext");
    let body = "#!/bin/sh\ncase \"$1\" in\n  --haj-describe) echo 入れる; exit 0 ;;\n  --haj-help) echo 入れ方の詳細; exit 0 ;;\n  --haj-complete) shift; [ $# -eq 0 ] && printf '%s\\n' locale-ja theme; exit 0 ;;\n  --haj-env) echo 'EXT_DIR=/tmp/x'; exit 0 ;;\nesac\necho RUN\n";
    let dir = sb.dir.join("remote/ext/commands");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("install"), body).unwrap();
    fs::set_permissions(dir.join("install"), fs::Permissions::from_mode(0o755)).unwrap();
    sb.write("remote/ext/config", "name = ext\nexpose = namespace\n");
    commit_all(&remote, "ext");

    let out = sb.haj(&sb.dir, cp, &["tree", "install", remote.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "install 失敗: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // help / env / which の合成形
    let help = stdout(&sb.haj(&sb.dir, cp, &["help", "ext", "install"]));
    assert!(
        help.contains("入れ方の詳細"),
        "--haj-help が出ない:\n{help}"
    );
    let env = stdout(&sb.haj(&sb.dir, cp, &["env", "ext", "install"]));
    assert!(env.contains("EXT_DIR=/tmp/x"), "--haj-env が出ない:\n{env}");
    let which = stdout(&sb.haj(&sb.dir, cp, &["which", "ext", "install"]));
    assert!(
        which.contains("commands/install"),
        "which がパスを見せない:\n{which}"
    );

    // 補完: ツリー名は打てる名前として出る。haj ext <TAB> は一覧、以降は転送
    let comp = stdout(&sb.haj(&sb.dir, cp, &["__complete"]));
    assert!(comp.contains("ext\t"), "ツリー名が補完に出ない:\n{comp}");
    assert!(
        !comp.contains("install\t"),
        "namespace のコマンドが素の補完に漏れている:\n{comp}"
    );
    let comp = stdout(&sb.haj(&sb.dir, cp, &["__complete", "ext"]));
    assert!(comp.contains("install\t入れる"), "一覧が出ない:\n{comp}");
    let comp = stdout(&sb.haj(&sb.dir, cp, &["__complete", "ext", "install"]));
    assert!(comp.contains("locale-ja"), "転送されない:\n{comp}");

    // help に入口の1行が出る(コマンドは並べない)
    let help = stdout(&sb.haj(&sb.dir, cp, &["help"]));
    assert!(
        help.contains("ツリー名前空間") && help.contains("haj ext で一覧"),
        "help に入口が無い:\n{help}"
    );
}

#[test]
fn ツリー名は探索より手前でエイリアスより後() {
    let sb = Sandbox::new("tree-ns-order");
    let cp = sb.path("syscmds");

    let remote = git_remote(&sb, "remote/tools");
    sb.command(
        "remote/tools",
        "greet",
        &conforming("あいさつ", "", "", "echo TREE_GREET"),
    );
    commit_all(&remote, "tools");
    let out = sb.haj(
        &sb.dir,
        cp.to_str().unwrap(),
        &["tree", "install", remote.to_str().unwrap()],
    );
    assert!(out.status.success());

    // 共通スコープに tools という名前のコマンドを置いても、ツリー名前空間が勝つ
    sb.command("sys", "tools", "#!/bin/sh\necho FLAT_CMD\n");
    let syscp = sb.path("sys/commands");
    let out = sb.haj(&sb.dir, syscp.to_str().unwrap(), &["tools"]);
    assert!(
        stdout(&out).contains("greet"),
        "ツリー名が探索に負けている:\n{}",
        stdout(&out)
    );

    // エイリアスはツリー名より勝つ
    sb.write(
        "xdgconf/haj/config",
        "alias.tools = sh -- echo ALIAS_WINS\n",
    );
    let out = Command::new(env!("CARGO_BIN_EXE_haj"))
        .args(["tools"])
        .current_dir(&sb.dir)
        .env("HAJ_COMMAND_PATH", sb.path("nonexistent"))
        .env("HAJ_NO_CACHE", "1")
        .env("HOME", &sb.dir)
        .env("XDG_CONFIG_HOME", sb.path("xdgconf"))
        .output()
        .unwrap();
    assert_eq!(
        stdout(&out).trim(),
        "ALIAS_WINS",
        "エイリアスがツリー名に負けている"
    );
}

// ---- env の集約(SPEC §9.6 / §9.7): 名前を省くと全コマンドの --haj-env を連結 ----

#[test]
fn envは名前を省くと全コマンドのhaj_envを節で連結する() {
    let sb = Sandbox::new("env-aggregate");
    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    // ツリー: install は対応、update は非対応(黙って飛ばす)
    let remote = git_remote(&sb, "remote/ext");
    let with_env = "#!/bin/sh\ncase \"$1\" in\n  --haj-describe) echo x; exit 0 ;;\n  --haj-env) printf '%s\\n' '# 配置先' 'EXT_DIR=/tmp/x'; exit 0 ;;\nesac\n";
    let without_env =
        "#!/bin/sh\ncase \"$1\" in --haj-describe) echo y; exit 0 ;; esac\necho RUN\n";
    let dir = sb.dir.join("remote/ext/commands");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("install"), with_env).unwrap();
    fs::write(dir.join("update"), without_env).unwrap();
    for n in ["install", "update"] {
        fs::set_permissions(dir.join(n), fs::Permissions::from_mode(0o755)).unwrap();
    }
    sb.write("remote/ext/config", "name = ext\nexpose = namespace\n");
    commit_all(&remote, "ext");
    let out = sb.haj(&sb.dir, cp, &["tree", "install", remote.to_str().unwrap()]);
    assert!(out.status.success());

    let env = stdout(&sb.haj(&sb.dir, cp, &["env", "ext"]));
    assert!(
        env.contains("# ==== install ====") && env.contains("EXT_DIR=/tmp/x"),
        "ツリーの集約が出ない:\n{env}"
    );
    assert!(
        !env.contains("==== update ===="),
        "非対応コマンドの節が出ている:\n{env}"
    );

    // タスク: haj env run(名前なし)も同じ連結
    let tdir = sb.dir.join("proj/.haj/tasks");
    fs::create_dir_all(&tdir).unwrap();
    fs::write(tdir.join("build"), with_env).unwrap();
    fs::set_permissions(tdir.join("build"), fs::Permissions::from_mode(0o755)).unwrap();
    let env = stdout(&sb.haj(&sb.path("proj"), cp, &["env", "run"]));
    assert!(
        env.contains("# ==== build ====") && env.contains("EXT_DIR=/tmp/x"),
        "タスクの集約が出ない:\n{env}"
    );

    // どれも対応していなければエラー
    let out = sb.haj(&sb.dir, cp, &["env", "run"]);
    assert_eq!(out.status.code(), Some(1), "外で run 集約が通っている");
}

#[test]
fn ツリー名前空間の中でも予約語の名前が使える() {
    // haj new tree のような自然な名前を、予約語 (tree) が塞がないこと (§9.7)
    let sb = Sandbox::new("tree-ns-reserved");
    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    let remote = git_remote(&sb, "remote/gen");
    sb.command(
        "remote/gen",
        "tree",
        &conforming("骨組みを作る", "", "", "echo TREE_GEN"),
    );
    sb.write("remote/gen/config", "name = gen\nexpose = namespace\n");
    commit_all(&remote, "gen");
    let out = sb.haj(&sb.dir, cp, &["tree", "install", remote.to_str().unwrap()]);
    assert!(out.status.success());

    let out = sb.haj(&sb.dir, cp, &["gen", "tree"]);
    assert_eq!(
        stdout(&out).trim(),
        "TREE_GEN",
        "名前空間で予約語名のコマンドが呼べない: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let list = stdout(&sb.haj(&sb.dir, cp, &["gen"]));
    assert!(list.contains("tree"), "一覧に出ない:\n{list}");

    // 素の形では従来どおり予約語は組み込み (haj tree list が動く)
    let out = sb.haj(&sb.dir, cp, &["tree", "list"]);
    assert!(stdout(&out).contains("gen"), "組み込み tree が奪われた");
}

#[test]
fn sigpipeは既定に戻してから実行される() {
    // Rust 既定の「SIGPIPE 無視」を exec した子に継がせない。
    // 無視のままだと kill -13 は no-op になり exit 0 で戻ってくる。
    use std::os::unix::process::ExitStatusExt;
    let sb = Sandbox::new("sigpipe");
    let cp = sb.path("nonexistent");
    let out = sb.haj(&sb.dir, cp.to_str().unwrap(), &["sh", "kill -13 $$"]);
    assert_eq!(
        out.status.signal(),
        Some(13),
        "SIGPIPE が既定に戻っていない (exit: {:?})",
        out.status.code()
    );
}

#[test]
fn 起動ディレクトリとユーザー設定パスが渡る() {
    let sb = Sandbox::new("runtime-env");
    sb.command(
        "sys",
        "where",
        &conforming(
            "位置",
            "",
            "",
            "echo \"start=$HAJ_START_DIR cfg=$HAJ_USER_CONFIG\"",
        ),
    );
    fs::create_dir_all(sb.path("proj/.haj")).unwrap();
    sb.write("proj/.haj/config", "name = myapp\n");

    let cp = sb.path("sys/commands");
    let cp = cp.to_str().unwrap();

    // getcwd は symlink を解決するので、期待値も canonicalize で合わせる
    let start = sb.dir.canonicalize().unwrap().display().to_string();

    // -C 無し: HAJ_START_DIR は cwd と一致する
    let out = sb.haj(&sb.dir, cp, &["where"]);
    let expect_cfg = sb.path(".config/haj/config").display().to_string();
    assert_eq!(
        stdout(&out).trim(),
        format!("start={start} cfg={expect_cfg}"),
        "実行時変数が注入されていない"
    );

    // -C あり: HAJ_START_DIR は移動**前**の場所を指す(cwd は移動後)
    let out = sb.haj(&sb.dir, cp, &["-C", "proj", "where"]);
    assert_eq!(
        stdout(&out).trim(),
        format!("start={start} cfg={expect_cfg}"),
        "-C 適用前の起動ディレクトリになっていない"
    );

    // 呼び出し元の残留値は上書きされる(親 haj の値を継がない)
    let out = Command::new(env!("CARGO_BIN_EXE_haj"))
        .args(["where"])
        .current_dir(&sb.dir)
        .env("HAJ_COMMAND_PATH", cp)
        .env("HAJ_NO_CACHE", "1")
        .env("HOME", &sb.dir)
        .env_remove("XDG_CONFIG_HOME")
        .env("HAJ_START_DIR", "/stale/value")
        .output()
        .unwrap();
    assert!(
        stdout(&out).contains(&format!(
            "start={}",
            sb.dir.canonicalize().unwrap().display()
        )),
        "残留した HAJ_START_DIR が上書きされていない: {}",
        stdout(&out)
    );
}

#[test]
fn 規約フックにも実行時変数が渡る() {
    let sb = Sandbox::new("runtime-env-hook");
    sb.command(
        "sys",
        "envy",
        "#!/bin/sh\ncase \"$1\" in\n  --haj-describe) echo \"env実験\"; exit 0 ;;\n  --haj-env) echo \"SEEN=$HAJ_START_DIR\"; exit 0 ;;\nesac\n",
    );
    let cp = sb.path("sys/commands");
    let out = sb.haj(&sb.dir, cp.to_str().unwrap(), &["env", "envy"]);
    assert!(
        stdout(&out).contains(&format!(
            "SEEN={}",
            sb.dir.canonicalize().unwrap().display()
        )),
        "--haj-env フックに HAJ_START_DIR が渡っていない: {}",
        stdout(&out)
    );
}

#[test]
fn sh委譲にも実行時変数が渡る() {
    let sb = Sandbox::new("runtime-env-sh");
    let cp = sb.path("nonexistent");
    let out = sb.haj(
        &sb.dir,
        cp.to_str().unwrap(),
        &["sh", "--", "echo", "S=$HAJ_START_DIR"],
    );
    assert!(
        stdout(&out).contains(&format!("S={}", sb.dir.canonicalize().unwrap().display())),
        "sh 委譲に HAJ_START_DIR が渡っていない: {}",
        stdout(&out)
    );
}

// ---- HAJ_TREE(SPEC §3.1): ツリー多重インストールのインスタンス名 ----

/// インストール済みツリーのフィクスチャを git 無しで置く(install は clone する
/// だけなので、置き場に直接作っても等価 — docs/trees.md の COPY と同じ理屈)。
fn installed_tree(sb: &Sandbox, name: &str, cmd_name: &str, body: &str) {
    let rel = format!(".local/share/haj/trees/{name}");
    sb.command(&rel, cmd_name, body);
}

#[test]
fn インストール済みツリーのコマンドにhaj_treeが渡る() {
    let sb = Sandbox::new("haj-tree");
    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    installed_tree(
        &sb,
        "tools",
        "who",
        &conforming("名乗る", "", "", r#"echo "TREE=${HAJ_TREE:-unset}""#),
    );

    // 素の探索(flat)でも名前空間の明示形でも、同じインストール名が渡る
    let flat = sb.haj(&sb.dir, cp, &["who"]);
    assert_eq!(
        stdout(&flat).trim(),
        "TREE=tools",
        "flat 探索で HAJ_TREE が渡らない"
    );
    let ns = sb.haj(&sb.dir, cp, &["tools", "who"]);
    assert_eq!(
        stdout(&ns).trim(),
        "TREE=tools",
        "名前空間形で HAJ_TREE が渡らない"
    );
}

#[test]
fn 多重インストールでインスタンスごとに別のhaj_treeが渡る() {
    let sb = Sandbox::new("haj-tree-multi");
    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    // 同じ中身のツリーを別名で2つ(--name a / --name b の多重インストール相当)
    let body = conforming(
        "状態の置き場を名乗る",
        "",
        "",
        r#"echo "STATE=$HOME/.local/state/myext/${HAJ_TREE:-default}""#,
    );
    installed_tree(&sb, "work", "state", &body);
    installed_tree(&sb, "home", "state", &body);

    let a = stdout(&sb.haj(&sb.dir, cp, &["work", "state"]));
    let b = stdout(&sb.haj(&sb.dir, cp, &["home", "state"]));
    assert!(
        a.trim().ends_with("/myext/work"),
        "work の状態パスが分かれない: {a}"
    );
    assert!(
        b.trim().ends_with("/myext/home"),
        "home の状態パスが分かれない: {b}"
    );
}

#[test]
fn ツリー以外のコマンドではhaj_treeが未設定になる() {
    let sb = Sandbox::new("haj-tree-unset");
    sb.command(
        "sys",
        "who",
        &conforming("名乗る", "", "", r#"echo "TREE=${HAJ_TREE:-unset}""#),
    );
    sb.command(
        "proj/.haj",
        "pwho",
        &conforming("名乗る", "", "", r#"echo "TREE=${HAJ_TREE:-unset}""#),
    );
    let cp = sb.path("sys/commands");
    let cp = cp.to_str().unwrap();

    // 呼び出し元の環境に HAJ_TREE が残っていても、明示的に消される
    let run = |args: &[&str], cwd: &Path| {
        Command::new(env!("CARGO_BIN_EXE_haj"))
            .args(args)
            .current_dir(cwd)
            .env("HAJ_COMMAND_PATH", cp)
            .env("HAJ_NO_CACHE", "1")
            .env("HOME", &sb.dir)
            .env_remove("XDG_CONFIG_HOME")
            .env("HAJ_TREE", "stale") // 前のプロセスの残骸を模す
            .output()
            .unwrap()
    };

    let sys = run(&["who"], &sb.dir);
    assert_eq!(
        stdout(&sys).trim(),
        "TREE=unset",
        "共通コマンドに HAJ_TREE が漏れた"
    );
    let proj = run(&["pwho"], &sb.path("proj"));
    assert_eq!(
        stdout(&proj).trim(),
        "TREE=unset",
        "プロジェクトのコマンドに HAJ_TREE が漏れた"
    );
    // haj exec(外の世界)にも渡さない
    let ex = run(
        &["exec", "sh", "-c", "echo TREE=${HAJ_TREE:-unset}"],
        &sb.dir,
    );
    assert_eq!(
        stdout(&ex).trim(),
        "TREE=unset",
        "haj exec に HAJ_TREE が漏れた"
    );
}

#[test]
fn 規約フックにもhaj_treeが渡る() {
    let sb = Sandbox::new("haj-tree-hook");
    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    installed_tree(
        &sb,
        "tools",
        "who",
        r#"#!/bin/sh
case "$1" in
  --haj-describe) echo "tree=${HAJ_TREE:-unset}"; exit 0 ;;
esac
true
"#,
    );

    let list = stdout(&sb.haj(&sb.dir, cp, &["tools"]));
    assert!(
        list.contains("tree=tools"),
        "フックに HAJ_TREE が渡っていない:\n{list}"
    );
}

#[test]
fn storeは予約語で探索に奪われない() {
    let sb = Sandbox::new("store-reserved");
    sb.command(
        "proj/.haj",
        "store",
        &conforming("乗っ取り", "", "", "echo HIJACK"),
    );
    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    // 組み込みの使い方が出る(実行ファイルは無視される)
    let out = sb.haj(&sb.path("proj"), cp, &["store"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        !stdout(&out).contains("HIJACK"),
        ".haj/commands/store が実行された"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("使い方"),
        "組み込みの使い方が出ない: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // 一覧と補完にも組み込みとして出る
    let comp = stdout(&sb.haj(&sb.path("proj"), cp, &["__complete"]));
    assert!(comp.contains("store"), "補完に store が出ない:\n{comp}");
    let comp = stdout(&sb.haj(&sb.dir, cp, &["__complete", "store"]));
    assert!(
        comp.contains("get") && comp.contains("put") && comp.contains("status"),
        "store の動詞が補完されない:\n{comp}"
    );
}

#[test]
fn config_treeは設定とstoreに続けて実効envを節連結で出す() {
    let sb = Sandbox::new("config-tree-eff");
    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    // --haj-env に応答するコマンドを持つインストール済みツリー。
    // GREETING は tree設定で注入され、TOKEN_FILE はコマンド自身の既定値が出る
    installed_tree(
        &sb,
        "hajime",
        "greet",
        concat!(
            "#!/bin/sh\n",
            "case \"$1\" in\n",
            "  --haj-describe) echo あいさつ; exit 0 ;;\n",
            "  --haj-env)\n",
            "    echo \"GREETING=${GREETING:-hello}\"\n",
            "    echo \"TOKEN_FILE=${TOKEN_FILE:-/tmp/t}\"\n",
            "    exit 0 ;;\n",
            "esac\n",
        ),
    );
    sb.write(
        ".config/haj/config",
        "tree.hajime.env.GREETING = やあ\n\
         tree.hajime.secret.API_KEY = vault://secret/data/x/key\n",
    );

    let out = sb.haj(&sb.dir, cp, &["config", "--tree", "hajime"]);
    let s = stdout(&out);
    // 前半: 設定 (tree.*) と宣言
    assert!(
        s.contains("tree.hajime.env.GREETING"),
        "env 設定が出ない:\n{s}"
    );
    assert!(
        s.contains("tree.hajime.secret.API_KEY"),
        "宣言が出ない:\n{s}"
    );
    // 後半: 実効 env — --haj-env の節連結。tree設定の注入もコマンドの既定値も見える
    assert!(
        s.contains("# ==== greet ===="),
        "実効 env の節が出ない:\n{s}"
    );
    assert!(
        s.contains("GREETING=やあ"),
        "tree設定が実効 env に効いていない:\n{s}"
    );
    assert!(
        s.contains("TOKEN_FILE=/tmp/t"),
        "コマンド自身の既定値が出ない:\n{s}"
    );
    assert!(
        s.contains("haj env hajime"),
        "--env-file 用の素の形式への導線が無い:\n{s}"
    );
}

#[test]
fn config_treeは未インストールでも設定を出し実効envはその旨を言う() {
    let sb = Sandbox::new("config-tree-ghost");
    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    sb.write(
        ".config/haj/config",
        "tree.ghost.secret.API_KEY = vault://secret/data/x/key\n",
    );

    let out = sb.haj(&sb.dir, cp, &["config", "--tree", "ghost"]);
    let s = stdout(&out);
    assert!(
        s.contains("未インストール"),
        "未インストールの注記が無い:\n{s}"
    );
    assert!(
        s.contains("tree.ghost.secret.API_KEY"),
        "設定だけの状態でも宣言が出るべき:\n{s}"
    );
    assert!(
        s.contains("実効 env: (未インストールのため取得できません)"),
        "実効 env の不在理由が出ない:\n{s}"
    );
}

// ---- haj tree configure (SPEC §9.5 / §10.8 の入口) ----

fn haj_with_stdin(sb: &Sandbox, cp: &str, args: &[&str], input: &str) -> Output {
    use std::io::Write as _;
    let mut child = Command::new(env!("CARGO_BIN_EXE_haj"))
        .args(args)
        .current_dir(&sb.dir)
        .env("HAJ_COMMAND_PATH", cp)
        .env("HAJ_NO_CACHE", "1")
        .env("HOME", &sb.dir)
        .env_remove("XDG_CONFIG_HOME")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn tree_config_init(sb: &Sandbox, tree: &str, body: &str) {
    let rel = format!(".local/share/haj/trees/{tree}/config-init");
    sb.write(&rel, body);
    fs::set_permissions(sb.path(&rel), fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn tree_configureは提案を確認のうえ接頭辞を付けてユーザー設定へ追記する() {
    let sb = Sandbox::new("tree-configure");
    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    // HAJ_TREE が渡ることも値で確かめる(インストール名は出力に書かない規約)
    tree_config_init(
        &sb,
        "hajime",
        concat!(
            "#!/bin/sh\n",
            "echo \"# hajime の初期設定\"\n",
            "echo \"secret.API_KEY = vault://users/${HAJ_TREE}/key\"\n",
            "echo \"env.GREETING = やあ\"\n",
        ),
    );

    // y で追記される。コアが tree.<インストール名>. を付ける
    let out = haj_with_stdin(&sb, cp, &["tree", "configure", "hajime"], "y\n");
    let s = stdout(&out);
    assert!(out.status.success(), "configure 失敗:\n{s}");
    assert!(
        s.contains("tree.hajime.secret.API_KEY = vault://users/hajime/key"),
        "接頭辞付きの提案が出ない:\n{s}"
    );
    let cfg = fs::read_to_string(sb.path(".config/haj/config")).unwrap();
    assert!(
        cfg.contains("tree.hajime.secret.API_KEY = vault://users/hajime/key"),
        "宣言が追記されていない:\n{cfg}"
    );
    assert!(
        cfg.contains("tree.hajime.env.GREETING = やあ"),
        "env が追記されていない:\n{cfg}"
    );

    // 2回目はすべて設定済み — 何も書かない(冪等)
    let out = haj_with_stdin(&sb, cp, &["tree", "configure", "hajime"], "y\n");
    let s = stdout(&out);
    assert!(
        s.contains("提案はすべて設定済み"),
        "設定済みの案内が出ない:\n{s}"
    );
    let cfg2 = fs::read_to_string(sb.path(".config/haj/config")).unwrap();
    assert_eq!(cfg, cfg2, "2回目で設定が変わった");
}

#[test]
fn tree_configureはyでなければ何も書かない() {
    let sb = Sandbox::new("tree-configure-no");
    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    tree_config_init(&sb, "hajime", "#!/bin/sh\necho \"env.GREETING = やあ\"\n");

    let out = haj_with_stdin(&sb, cp, &["tree", "configure", "hajime"], "n\n");
    assert!(!out.status.success(), "n なのに成功扱い");
    assert!(
        stdout(&out).contains("中止しました"),
        "中止の案内が無い:\n{}",
        stdout(&out)
    );
    assert!(
        !sb.path(".config/haj/config").exists(),
        "n なのに設定ファイルが書かれた"
    );
}

#[test]
fn tree_configureは書式外の出力で何も書かずに止まる() {
    let sb = Sandbox::new("tree-configure-bad");
    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    tree_config_init(
        &sb,
        "hajime",
        "#!/bin/sh\necho \"tree.hajime.env.X = 自分で接頭辞を書いた\"\n",
    );

    let out = haj_with_stdin(&sb, cp, &["tree", "configure", "hajime"], "y\n");
    assert!(!out.status.success(), "書式外なのに成功した");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("書式外"), "書式エラーの案内が無い:\n{err}");
    assert!(
        !sb.path(".config/haj/config").exists(),
        "書式外なのに設定ファイルが書かれた"
    );
}

#[test]
fn tree_configureはconfig_initが無ければその旨を言う() {
    let sb = Sandbox::new("tree-configure-none");
    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    installed_tree(
        &sb,
        "plain",
        "who",
        &conforming("名乗る", "", "", "echo hi"),
    );

    let out = haj_with_stdin(&sb, cp, &["tree", "configure", "plain"], "");
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("初期設定を提案していません"),
        "config-init 不在の案内が無い:\n{err}"
    );
}

#[test]
fn installはconfig_initがあれば入口を案内する() {
    let sb = Sandbox::new("tree-configure-hint");
    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    let remote = git_remote(&sb, "remote/tools");
    sb.command(
        "remote/tools",
        "greet",
        &conforming("あいさつ", "", "", "echo HELLO"),
    );
    sb.write(
        "remote/tools/config-init",
        "#!/bin/sh\necho \"env.GREETING = やあ\"\n",
    );
    fs::set_permissions(
        sb.path("remote/tools/config-init"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    commit_all(&remote, "greet と config-init");

    let out = sb.haj(&sb.dir, cp, &["tree", "install", remote.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "install 失敗: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout(&out).contains("haj tree configure tools"),
        "config-init の入口案内が無い:\n{}",
        stdout(&out)
    );
}

// ---- haj config get / set (SPEC §8.5) ----

#[test]
fn config_setは追記しgetで取れて既存キーは論理行ごと置換される() {
    let sb = Sandbox::new("config-get-set");
    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    // 継続行を含む既存の設定。コメントと他のキーは保たれること
    sb.write(
        ".config/haj/config",
        concat!(
            "# 先頭のコメント\n",
            "meta.username = old-name\n",
            "alias.oci = --secret A=vault://x/a \\\n",
            "            exec oci\n",
        ),
    );

    // 新規キーの追記
    let out = sb.haj(
        &sb.dir,
        cp,
        &["config", "set", "meta.email", "a@example.com"],
    );
    assert!(
        out.status.success(),
        "set 失敗: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let got = stdout(&sb.haj(&sb.dir, cp, &["config", "get", "meta.email"]));
    assert_eq!(got.trim(), "a@example.com", "set した値が get で取れない");

    // 既存キーの置換
    let out = sb.haj(&sb.dir, cp, &["config", "set", "meta.username", "hajime"]);
    assert!(out.status.success());
    let cfg = fs::read_to_string(sb.path(".config/haj/config")).unwrap();
    assert!(
        cfg.contains("meta.username = hajime"),
        "置換されていない:\n{cfg}"
    );
    assert!(!cfg.contains("old-name"), "旧値が残っている:\n{cfg}");
    assert!(cfg.contains("# 先頭のコメント"), "コメントが消えた:\n{cfg}");
    assert!(cfg.contains("exec oci"), "他のキーの継続行が消えた:\n{cfg}");

    // 継続行を持つキーの置換は継続行ごと消える
    let out = sb.haj(&sb.dir, cp, &["config", "set", "alias.oci", "exec oci"]);
    assert!(out.status.success());
    let cfg = fs::read_to_string(sb.path(".config/haj/config")).unwrap();
    assert!(
        cfg.contains("alias.oci = exec oci"),
        "置換されていない:\n{cfg}"
    );
    assert!(
        !cfg.contains("vault://x/a"),
        "旧論理行の断片が残っている:\n{cfg}"
    );
}

#[test]
fn config_getはコア既知キーの実効値を出し未設定キーはexit1() {
    let sb = Sandbox::new("config-get-known");
    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    // 既知キー: 何も無ければ既定値
    let out = sb.haj(&sb.dir, cp, &["config", "get", "hook_timeout_ms"]);
    assert_eq!(stdout(&out).trim(), "2000", "既定値が出ない");

    // 環境変数が勝つ
    let out = Command::new(env!("CARGO_BIN_EXE_haj"))
        .args(["config", "get", "hook_timeout_ms"])
        .current_dir(&sb.dir)
        .env("HAJ_COMMAND_PATH", cp)
        .env("HAJ_NO_CACHE", "1")
        .env("HOME", &sb.dir)
        .env_remove("XDG_CONFIG_HOME")
        .env("HAJ_HOOK_TIMEOUT_MS", "5000")
        .output()
        .unwrap();
    assert_eq!(stdout(&out).trim(), "5000", "環境変数が勝たない");

    // 未設定の未知キーは exit 1
    let out = sb.haj(&sb.dir, cp, &["config", "get", "meta.nothing"]);
    assert!(!out.status.success(), "未設定なのに成功した");
}

#[test]
fn config_getはtokenの平文を出さず参照なら出す() {
    let sb = Sandbox::new("config-get-token");
    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    sb.write(
        ".config/haj/config",
        "selfupgrade.token = glpat-PLAINTEXT\n",
    );
    let out = sb.haj(&sb.dir, cp, &["config", "get", "selfupgrade.token"]);
    assert!(!out.status.success(), "平文トークンが出た");
    assert!(
        !stdout(&out).contains("glpat-PLAINTEXT"),
        "平文が stdout に漏れた"
    );

    sb.write(
        ".config/haj/config",
        "selfupgrade.token = vault://secret/data/haj/token\n",
    );
    let out = sb.haj(&sb.dir, cp, &["config", "get", "selfupgrade.token"]);
    assert_eq!(
        stdout(&out).trim(),
        "vault://secret/data/haj/token",
        "参照が出ない"
    );
}

// ---- haj help --quick(SPEC §5 / §9.5): quickref の連結 ----

#[test]
fn help_quickはコアと全ツリーのquickrefを連結しtreeを置換する() {
    let sb = Sandbox::new("quickref");
    let cp = sb.path("nonexistent");
    let cp = cp.to_str().unwrap();

    // quickref を持つツリー({TREE} プレースホルダ入り)
    installed_tree(
        &sb,
        "tools",
        "greet",
        &conforming("あいさつ", "", "", "true"),
    );
    sb.write(
        ".local/share/haj/trees/tools/docs/quickref.md",
        "haj {TREE} greet <名前>    あいさつする\n",
    );
    // quickref を持たないツリーは黙って飛ばす
    installed_tree(
        &sb,
        "plain",
        "noop",
        &conforming("なにもしない", "", "", "true"),
    );

    let out = sb.haj(&sb.dir, cp, &["help", "--quick"]);
    assert!(out.status.success());
    let s = stdout(&out);
    assert!(s.contains("# haj quickref"), "コアの節が無い:\n{s}");
    assert!(
        s.contains("haj secret get") && s.contains("haj store"),
        "コアの主要操作が無い:\n{s}"
    );
    assert!(s.contains("## tools"), "ツリーの見出しが無い:\n{s}");
    assert!(
        s.contains("haj tools greet <名前>"),
        "{{TREE}} が置換されていない:\n{s}"
    );
    assert!(!s.contains("{TREE}"), "プレースホルダが残っている:\n{s}");
    assert!(
        !s.contains("## plain"),
        "quickref の無いツリーが出ている:\n{s}"
    );

    // 補完に --quick が出る
    let comp = stdout(&sb.haj(&sb.dir, cp, &["__complete", "help"]));
    assert!(comp.contains("--quick"), "--quick が補完されない:\n{comp}");
}
