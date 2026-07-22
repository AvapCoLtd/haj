//! サブコマンドとの規約(SPEC.md「コマンド規約」)を実装する。
//!
//! コアはサブコマンドの中身を知らない。知りたいこと(説明・使い方・補完候補)は
//! すべてサブコマンド自身に聞く。聞き方がこの規約で、これがhajの中核。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command as Proc, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::discovery::Command;

pub const DESCRIBE: &str = "--haj-describe";
pub const HELP: &str = "--haj-help";
pub const COMPLETE: &str = "--haj-complete";
pub const ENV: &str = "--haj-env";

/// 規約フックの実行に許す時間。
///
/// 上限を設けないと、壊れたサブコマンド1本(入力待ちで固まる等)が
/// `haj help` とシェルのTAB補完を巻き添えにして固める。補完は人間が
/// キーを押すたびに走るので、ここが詰まるのは致命的に体験が悪い。
pub const DEFAULT_HOOK_TIMEOUT_MS: u64 = 2000;

/// haj が起動された時点の cwd(`-C` 適用**前**)。
///
/// `-C` は set_current_dir でその場で移動するため、記録しないと「ユーザーが
/// 元居た場所」は失われる。main() の先頭(フラグ処理より前)で record する。
static START_DIR: OnceLock<Option<PathBuf>> = OnceLock::new();

/// main() の先頭で呼ぶ。`-C` の適用より前でなければ意味がない。
pub fn record_start_dir() {
    let _ = START_DIR.set(std::env::current_dir().ok());
}

/// コアが渡す実行時変数のうち、探索結果に依存しないもの(SPEC §3.1)。
///
/// `HAJ_START_DIR` は起動時 cwd(`-C` 適用前)、`HAJ_USER_CONFIG` はユーザー設定
/// ファイルのパス(XDG 解決後。存在しなくても「読みに行く場所」を渡す)。
/// 呼び出し元の環境に残った古い値を継がせないため、解決できないときは明示的に消す。
pub fn apply_runtime_env(proc: &mut Proc) {
    match START_DIR.get().and_then(|d| d.as_ref()) {
        Some(dir) => {
            proc.env("HAJ_START_DIR", dir);
        }
        None => {
            proc.env_remove("HAJ_START_DIR");
        }
    }
    match crate::config::config_dir() {
        Some(dir) => {
            proc.env("HAJ_USER_CONFIG", dir.join("config"));
        }
        None => {
            proc.env_remove("HAJ_USER_CONFIG");
        }
    }
}

fn hook_timeout() -> Duration {
    let cfg = crate::config::Config::load();
    let (v, _) = cfg.get(
        "HAJ_HOOK_TIMEOUT_MS",
        "hook_timeout_ms",
        &DEFAULT_HOOK_TIMEOUT_MS.to_string(),
    );
    Duration::from_millis(v.parse().unwrap_or(DEFAULT_HOOK_TIMEOUT_MS))
}

/// 規約フックを呼んで stdout を返す。答えられない/失敗/タイムアウトなら None。
///
/// 規約に応答しないコマンド(単に `--haj-describe` を無視して本処理を始めるもの)を
/// 想定し、stderr は捨て、stdin は /dev/null に落とす。stdinを塞がないと、
/// 対話的なコマンドが端末を奪って固まる。
pub fn hook(cmd: &Command, args: &[&str]) -> Option<String> {
    let mut proc = Proc::new(&cmd.path);
    proc.args(args)
        .env("HAJ_NAME", &cmd.name)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    if let Some(root) = &cmd.root {
        proc.env("HAJ_ROOT", root);
    } else {
        // PATH上の haj-* にはツリーが無い。前のプロセスの値が漏れないよう明示的に消す。
        proc.env_remove("HAJ_ROOT");
    }
    // インストール名(HAJ_TREE。SPEC §3.1)は本体実行と同じ規則でフックにも注入する。
    if let crate::project::Origin::Tree(name) = &cmd.origin {
        proc.env("HAJ_TREE", name);
    } else {
        proc.env_remove("HAJ_TREE");
    }
    apply_runtime_env(&mut proc);

    let mut child = proc.spawn().ok()?;

    // タイムアウト付きで待つ。try_wait をバックオフしながらポーリングする
    // (スレッドとチャネルを使う実装より単純で、依存も増えない。
    //  規約フックの出力は数行なのでパイプが詰まる心配はない)。
    let deadline = Instant::now() + hook_timeout();
    let mut backoff = Duration::from_millis(1);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    };

    if !status.success() {
        return None;
    }

    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    Some(out)
}

/// 一行説明。`haj help` の一覧に使う。複数行返してきても1行目だけ採る。
pub fn describe(cmd: &Command) -> Option<String> {
    let out = hook(cmd, &[DESCRIBE])?;
    let line = out.lines().next()?.trim();
    (!line.is_empty()).then(|| line.to_string())
}

/// 詳しい使い方。実装していなければ一行説明にフォールバックする。
pub fn long_help(cmd: &Command) -> Option<String> {
    hook(cmd, &[HELP])
        .map(|s| s.trim_end().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| describe(cmd))
}

/// `--haj-env` — そのコマンドが読む環境変数(KEY=value)。未実装なら None。
///
/// 出力はそのまま --env-file に渡せる形式(SPEC §4.4)。規約を知らないコマンドは
/// このフラグを無視して**本処理の出力**を返してくるため、形の検証で見分ける:
/// 空行と `#` コメント以外の行がすべて `KEY=value` の形でなければ未実装とみなす。
pub fn env_vars(cmd: &Command) -> Option<String> {
    let out = hook(cmd, &[ENV])
        .map(|s| s.trim_end().to_string())
        .filter(|s| !s.is_empty())?;
    let conforms = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .all(|l| l.split_once('=').is_some_and(|(k, _)| !k.trim().is_empty()));
    conforms.then_some(out)
}

/// 補完候補。`words` は「そのコマンド以降に入力済みの語」。
pub fn complete(cmd: &Command, words: &[String]) -> Vec<String> {
    let mut args: Vec<&str> = vec![COMPLETE];
    args.extend(words.iter().map(String::as_str));
    hook(cmd, &args)
        .map(|out| {
            out.lines()
                .map(str::trim_end)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// help.header / help.footer を、探索順で最初に見つかったツリーから読む。
///
/// コマンド一覧は自動生成するので、ここに置くのは「コマンド以外の案内」だけ
/// (URL、環境変数の上書き方法、ファイルの置き場所など)。
pub fn fragment(kind: &str) -> Option<String> {
    for dir in crate::discovery::command_dirs() {
        let Some(root) = dir.path.parent() else {
            continue;
        };
        let f = root.join(format!("help.{kind}"));
        if let Ok(s) = std::fs::read_to_string(&f) {
            return Some(s);
        }
    }
    None
}

/// 実行ファイルの識別子(パス + 更新時刻 + サイズ)。説明文キャッシュの鍵に使う。
pub fn stamp(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Some(format!(
        "{}\t{}.{}\t{}",
        path.display(),
        mtime.as_secs(),
        mtime.subsec_nanos(),
        meta.len()
    ))
}
