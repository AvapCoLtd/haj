//! `haj docs` — 端末で読めるドキュメント(SPEC §9.3)。
//!
//! コマンドと同じ思想で**探索に乗る**。ツリーは `commands/` と並んで `docs/` を
//! 持てて、プロジェクトの手順書(onboarding 等)がそのプロジェクトの中でだけ生える。
//! haj 自身のドキュメントはバイナリに埋め込む(`include_str!`。依存ゼロのまま)ので、
//! 「コマンドの作り方」と契約の全文がどこでも引ける。
//!
//! 出力は素の markdown を stdout へ。ページャは使う側のパイプに任せる(コアは薄く)。

use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::project::Origin;

/// haj 同梱のドキュメント。リポジトリの docs/ と SPEC.md が埋め込み元を兼ねる
/// (repo でも端末でも同じものが読める)。
/// ツリー側に同名トピックが置かれたら**そちらが勝つ**。コマンドの予約語と違って
/// 奪われても実害が無く、上書きできるほうが探索の一貫性がある。
const EMBEDDED: &[(&str, &str, &str)] = &[
    (
        "writing-commands",
        "コマンドの作り方",
        include_str!("../docs/writing-commands.md"),
    ),
    (
        "secrets",
        "シークレットの受け渡し(--secret / --env-file / --secret-file)",
        include_str!("../docs/secrets.md"),
    ),
    (
        "trees",
        "ツリーの作り方と配布(haj tree)",
        include_str!("../docs/trees.md"),
    ),
    (
        "spec",
        "haj 仕様(コアとサブコマンドの契約の全文)",
        include_str!("../SPEC.md"),
    ),
];

/// 見つかったトピック1つ。
struct Topic {
    name: String,
    describe: String,
    origin: Origin,
    /// ツリー由来ならパス。埋め込みなら None(EMBEDDED から引く)。
    path: Option<PathBuf>,
}

/// 使えるトピックを探索順で全部集める。同名は先勝ち(埋め込みは最後)。
fn list() -> Vec<Topic> {
    let mut found: Vec<Topic> = Vec::new();

    for (root, origin) in crate::discovery::doc_trees() {
        // docs/ は commands/ の隣。commands/ が無いツリー(docs だけ置く)も正当。
        let Ok(entries) = std::fs::read_dir(root.join("docs")) else {
            continue;
        };
        let mut names: Vec<(String, PathBuf)> = entries
            .flatten()
            .filter_map(|e| {
                let f = e.file_name().into_string().ok()?;
                let name = f.strip_suffix(".md")?.to_string();
                (!name.is_empty() && !name.starts_with('.')).then(|| (name, e.path()))
            })
            .collect();
        names.sort();
        for (name, path) in names {
            if found.iter().any(|t: &Topic| t.name == name) {
                continue; // 探索順で先勝ち
            }
            found.push(Topic {
                describe: first_heading(&path),
                name,
                origin: origin.clone(),
                path: Some(path),
            });
        }
    }

    for (name, describe, _) in EMBEDDED {
        if !found.iter().any(|t| t.name == *name) {
            found.push(Topic {
                name: name.to_string(),
                describe: describe.to_string(),
                origin: Origin::Core,
                path: None,
            });
        }
    }

    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// 一覧の説明はファイル先頭の見出し行から取る(`# タイトル` の `タイトル`)。
/// 規約フックの docs 版 — 別ファイルに説明を書かせない。
fn first_heading(path: &std::path::Path) -> String {
    let Ok(content) = std::fs::read_to_string(path) else {
        return String::new();
    };
    content
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim_start_matches('#').trim().to_string())
        .unwrap_or_default()
}

/// トピックの本文を読む(ツリー由来ならファイル、同梱なら埋め込み)。
fn content_of(t: &Topic) -> Result<String, String> {
    match &t.path {
        Some(p) => {
            std::fs::read_to_string(p).map_err(|e| format!("{} を読めません: {e}", p.display()))
        }
        None => Ok(EMBEDDED
            .iter()
            .find(|(n, _, _)| *n == t.name)
            .map(|(_, _, c)| (*c).to_string())
            .unwrap_or_default()),
    }
}

/// fzf 選択の結果。
enum Pick {
    /// 選択UIを出せない環境(非TTY・fzf不在など)。従来の一覧印字に落ちる
    Unavailable,
    /// ユーザーが選ばずに閉じた(Esc 等)。何もせず正常終了する
    Cancelled,
    Chosen(String),
}

/// 引数なしの `haj docs` を、端末では fzf の選択UIにする(SPEC §9.3)。
/// fzf は CLI への委譲(op / bao / git と同じ流儀)。stdout がパイプ・リダイレクト
/// のときは UI を出さないので、スクリプトからの利用は従来と変わらない。
/// UIコマンド・追加引数・プレビューのフィルタは設定で差し替えられる(§8.3)。
fn pick_with_fzf(topics: &[Topic]) -> Pick {
    if !std::io::stdout().is_terminal() {
        return Pick::Unavailable;
    }

    let cfg = crate::config::Config::load();
    let (fzf_cmd, _) = cfg.get("HAJ_DOCS_FZF_CMD", "docs.fzf_cmd", "fzf");
    let (fzf_args, _) = cfg.get("HAJ_DOCS_FZF_ARGS", "docs.fzf_args", "");
    let (preview_cmd, _) = cfg.get("HAJ_DOCS_PREVIEW_CMD", "docs.preview_cmd", "");

    // プレビューは自分自身に聞く(`haj docs <トピック>`)。開発中のバイナリが
    // PATH に居なくても動くよう current_exe を使う。引用できないパスなら
    // PATH の haj に任せる。
    let me = std::env::current_exe()
        .ok()
        .and_then(|p| p.to_str().map(String::from))
        .filter(|s| !s.contains('\''))
        .unwrap_or_else(|| "haj".to_string());

    // docs.preview_cmd はプレビューを markdown レンダラへ通すフィルタ(本文を stdin で受ける)
    let preview = if preview_cmd.trim().is_empty() {
        format!("'{me}' docs {{1}}")
    } else {
        format!("'{me}' docs {{1}} | {preview_cmd}")
    };

    let mut words = fzf_cmd.split_whitespace();
    let Some(bin) = words.next() else {
        return Pick::Unavailable;
    };
    let mut ui = Command::new(bin);
    ui.args(words)
        .arg("--delimiter=\t")
        .arg("--prompt=haj docs> ")
        .arg(format!("--preview={preview}"))
        .arg("--preview-window=right,70%,wrap");
    // 追加引数は haj の既定の**後ろ**に付ける。fzf は後勝ちなので、
    // --preview-window 等を設定で上書きできる
    ui.args(fzf_args.split_whitespace());

    let Ok(mut child) = ui.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn() else {
        return Pick::Unavailable; // UIコマンドが無い
    };

    if let Some(mut stdin) = child.stdin.take() {
        for t in topics {
            let _ = writeln!(stdin, "{}\t{}  {}", t.name, t.describe, t.origin.label());
        }
    }
    let Ok(out) = child.wait_with_output() else {
        return Pick::Unavailable;
    };
    let sel = String::from_utf8_lossy(&out.stdout);
    match sel.lines().next().and_then(|l| l.split('\t').next()) {
        Some(name) if !name.is_empty() => Pick::Chosen(name.to_string()),
        _ => Pick::Cancelled,
    }
}

/// Enter で開くビューア。`docs.pager` > `$PAGER` > `less` の順で決める。
/// 起動できなければ false(呼び出し元が print する)。
/// 値は空白で語分割するだけ(シェル解釈はしない — vault_login の引数と同じ流儀)。
fn show_with_pager(content: &str) -> bool {
    let (pager, _) = crate::config::Config::load().get("HAJ_DOCS_PAGER", "docs.pager", "");
    let pager = if pager.trim().is_empty() {
        std::env::var("PAGER").unwrap_or_default()
    } else {
        pager
    };
    let pager = if pager.trim().is_empty() {
        "less".to_string()
    } else {
        pager
    };
    let mut words = pager.split_whitespace();
    let Some(bin) = words.next() else {
        return false;
    };
    let Ok(mut child) = Command::new(bin).args(words).stdin(Stdio::piped()).spawn() else {
        return false;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(content.as_bytes());
    }
    child.wait().is_ok()
}

/// `haj docs` / `haj docs <トピック>`
pub fn run(args: &[String]) -> ! {
    let Some(topic) = args.first() else {
        let topics = list();
        if topics.is_empty() {
            println!("ドキュメントがありません。");
            std::process::exit(0);
        }

        // 端末なら fzf の選択UI(SPEC §9.3)。出せない環境では一覧印字に落ちる
        match pick_with_fzf(&topics) {
            Pick::Chosen(name) => {
                let Some(t) = topics.iter().find(|t| t.name == name) else {
                    std::process::exit(1); // fzfの候補は list() 由来なので来ないはず
                };
                match content_of(t) {
                    Ok(c) => {
                        if !show_with_pager(&c) {
                            print!("{c}");
                        }
                        std::process::exit(0);
                    }
                    Err(e) => {
                        eprintln!("haj: {e}");
                        std::process::exit(1);
                    }
                }
            }
            Pick::Cancelled => std::process::exit(0),
            Pick::Unavailable => {}
        }

        println!(" ドキュメント (haj docs <トピック> で表示):");
        let width = topics.iter().map(|t| t.name.len()).max().unwrap_or(8);
        for t in &topics {
            println!(
                "   {:width$}  {}  {}",
                t.name,
                t.describe,
                t.origin.label(),
                width = width
            );
        }
        std::process::exit(0);
    };

    let topics = list();
    let Some(t) = topics.iter().find(|t| t.name == *topic) else {
        eprintln!("haj: 未知のトピックです: {topic} (一覧: haj docs)");
        std::process::exit(1);
    };

    match content_of(t) {
        Ok(content) => {
            print!("{content}");
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("haj: {e}");
            std::process::exit(1);
        }
    }
}

/// `haj docs <TAB>` の補完候補。
pub fn complete(words: &[String]) -> Vec<String> {
    if words.is_empty() {
        list().into_iter().map(|t| t.name).collect()
    } else {
        Vec::new()
    }
}
