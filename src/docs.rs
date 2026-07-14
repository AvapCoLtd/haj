//! `haj docs` — 端末で読めるドキュメント(SPEC §9.3)。
//!
//! コマンドと同じ思想で**探索に乗る**。ツリーは `commands/` と並んで `docs/` を
//! 持てて、プロジェクトの手順書(onboarding 等)がそのプロジェクトの中でだけ生える。
//! haj 自身のドキュメントはバイナリに埋め込む(`include_str!`。依存ゼロのまま)ので、
//! 「コマンドの作り方」と契約の全文がどこでも引ける。
//!
//! 出力は素の markdown を stdout へ。ページャは使う側のパイプに任せる(コアは薄く)。

use std::path::PathBuf;

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

/// `haj docs` / `haj docs <トピック>`
pub fn run(args: &[String]) -> ! {
    let Some(topic) = args.first() else {
        let topics = list();
        if topics.is_empty() {
            println!("ドキュメントがありません。");
        } else {
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
        }
        std::process::exit(0);
    };

    let topics = list();
    let Some(t) = topics.iter().find(|t| t.name == *topic) else {
        eprintln!("haj: 未知のトピックです: {topic} (一覧: haj docs)");
        std::process::exit(1);
    };

    let content = match &t.path {
        Some(p) => match std::fs::read_to_string(p) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("haj: {} を読めません: {e}", p.display());
                std::process::exit(1);
            }
        },
        None => EMBEDDED
            .iter()
            .find(|(n, _, _)| n == topic)
            .map(|(_, _, c)| c.to_string())
            .unwrap_or_default(),
    };
    print!("{content}");
    std::process::exit(0);
}

/// `haj docs <TAB>` の補完候補。
pub fn complete(words: &[String]) -> Vec<String> {
    if words.is_empty() {
        list().into_iter().map(|t| t.name).collect()
    } else {
        Vec::new()
    }
}
