//! `haj secret` — 宣言を引く(SPEC §10.9)。読みだけ。
//!
//! サブコマンドが金庫を直接読むと、接続と認証の知識がツリーごとに複製される。
//! `haj secret` はその口を一つにする — ただし解決するのは**宣言表(§10.8 —
//! `tree.<名前>.secret.*`)にある参照だけ**。KEY で引き、参照は受けない。
//! 何を読めるかは宣言表が決める(capability)— ツリーのコードには物理パスも
//! `store://` も書かれず、宣言表を迂回する口が無い。
//!
//! 所有の規律: **secret = 読む(他所の物も含む)、store = 読み書き(自分の物
//! だけ)**。書きたい秘密は自分の store(§10.10)に置く。

use crate::secrets::Delivery;

const USAGE: &str = "\
使い方: haj secret get <KEY>    宣言 tree.<HAJ_TREE>.secret.<KEY> を解決して stdout へ
        haj secret list         宣言の一覧 (KEY=<参照>。値は解決しない)
        haj secret check        宣言と受け渡しの検証 (金庫に触らない)";

pub fn run(args: &[String], deliveries: &[Delivery]) -> ! {
    match args.split_first().map(|(a, r)| (a.as_str(), r)) {
        Some(("get", r)) => get(r),
        Some(("list", _)) => list(),
        Some(("check", _)) => check(deliveries),
        _ => die(USAGE),
    }
}

/// 文脈(自分の環境の `HAJ_TREE`)。ツリーの外ではエラー。
fn tree_ctx() -> Option<String> {
    std::env::var("HAJ_TREE").ok().filter(|t| !t.is_empty())
}

fn no_context() -> ! {
    die(
        "haj secret はツリーのコマンドの中でだけ使えます (HAJ_TREE が無い)。\n  \
         人手の点検なら HAJ_TREE=<インストール名> を明示して実行する (SPEC §10.10)",
    );
}

/// 宣言表(`tree.<名前>.secret.*`)。**ユーザー設定からだけ**読む(§10.8)。
fn declarations(tree: &str) -> Vec<(String, String)> {
    crate::config::Config::load().tree_entries(tree, "secret")
}

fn get(args: &[String]) -> ! {
    let Some(key) = args.first() else {
        die(USAGE);
    };
    let Some(tree) = tree_ctx() else {
        no_context();
    };
    let decls = declarations(&tree);
    let Some((_, reference)) = decls.iter().find(|(k, _)| k == key) else {
        // 宣言に無い KEY はエラー(capability)。宣言済みを列挙して案内する。
        let listed = if decls.is_empty() {
            format!(
                "(宣言はありません。~/.config/haj/config に tree.{tree}.secret.{key} = <参照> を書く)"
            )
        } else {
            let names: Vec<&str> = decls.iter().map(|(k, _)| k.as_str()).collect();
            format!("宣言済み: {}", names.join(", "))
        };
        die(&format!(
            "{key} は宣言されていません (tree.{tree}.secret.{key})\n  {listed}"
        ));
    };
    if !crate::secrets::is_reference(reference) {
        die(&plaintext_err(&tree, key));
    }
    match crate::secrets::expand(reference, false, Some(&tree)) {
        Ok(v) => {
            // 値そのもの+改行1つ($(...) が改行を落とす)。§10.4 と同じ末尾規則
            println!("{}", v.unwrap_or_else(|| reference.clone()));
            std::process::exit(0);
        }
        Err(e) => die(&format!("tree.{tree}.secret.{key}: {e}")),
    }
}

/// 平文の宣言はエラー — 秘密の平文を設定ファイルに書かせない(§10.8)。
fn plaintext_err(tree: &str, key: &str) -> String {
    format!(
        "tree.{tree}.secret.{key}: 参照ではありません。\n  \
         秘密の平文は設定ファイルに書かない — 平文の設定なら tree.{tree}.env.{key} に"
    )
}

fn list() -> ! {
    let Some(tree) = tree_ctx() else {
        no_context();
    };
    let decls = declarations(&tree);
    if decls.is_empty() {
        println!("宣言はありません (tree.{tree}.secret.*)。");
        println!("  ~/.config/haj/config に tree.{tree}.secret.<KEY> = <参照> を書く");
        std::process::exit(0);
    }
    // 参照は秘密ではない(§10.6)。値は解決しない。
    for (k, v) in decls {
        println!("{k}={v}");
    }
    std::process::exit(0);
}

/// `haj secret check` — 何が渡り、何が宣言されているのかを**解決せずに**確かめる
/// (SPEC §10.6)。金庫に問い合わせないので、ログインもタッチ認証も起きない。
fn check(deliveries: &[Delivery]) -> ! {
    let mut failed = false;
    let mut printed = false;

    // 受け渡しフラグの事前確認(旧 haj secrets --check)
    if !deliveries.is_empty() {
        println!(" 実行時に渡るもの (値は解決していません):");
        for d in deliveries {
            match d.plan() {
                Ok(rows) => {
                    for (kind, name, value) in rows {
                        let mark = if crate::secrets::is_reference(&value) {
                            "→"
                        } else {
                            " "
                        };
                        let note = value
                            .strip_prefix("store://")
                            .map(crate::store::check_note)
                            .unwrap_or_default();
                        println!("   {kind:10}  {name:20}  {mark} {value}{note}");
                    }
                }
                Err(e) => {
                    eprintln!("haj: {e}");
                    std::process::exit(1);
                }
            }
        }
        println!("\n (→ が付いたものが展開されます。他は平文としてそのまま渡ります)");
        printed = true;
    }

    // 宣言の検証(ツリー文脈があるとき)。写像は手元の設定だけで決まる。
    if let Some(tree) = tree_ctx() {
        let decls = declarations(&tree);
        if printed {
            println!();
        }
        if decls.is_empty() {
            println!(" 宣言 (tree.{tree}.secret.*): ありません");
        } else {
            println!(" 宣言 (tree.{tree}.secret.*):");
            let width = decls.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
            for (k, v) in &decls {
                if crate::secrets::is_reference(v) {
                    let note = v
                        .strip_prefix("store://")
                        .map(crate::store::check_note)
                        .unwrap_or_default();
                    println!("   {k:width$}  → {v}{note}");
                } else {
                    println!(
                        "   {k:width$}  ✗ 参照ではありません (平文の設定は tree.{tree}.env.{k} に)"
                    );
                    failed = true;
                }
            }
        }
        printed = true;
    }

    if !printed {
        println!("確かめるものがありません。");
        println!("  受け渡し: haj --secret KEY=<参照> ... secret check");
        println!(
            "  宣言:     ツリーのコマンドの中で haj secret check (人手なら HAJ_TREE=<名前> を明示)"
        );
    }
    std::process::exit(if failed { 1 } else { 0 });
}

/// 補完(builtin::complete から呼ばれる)。`get` には宣言済みの KEY —
/// 目録は手元の設定だけで列挙できる(金庫には触らない。SPEC §10.9)。
pub fn complete(words: &[String]) -> Vec<String> {
    match words.first().map(String::as_str) {
        None => ["get", "list", "check"]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        Some("get") if words.len() == 1 => match tree_ctx() {
            Some(tree) => declarations(&tree).into_iter().map(|(k, _)| k).collect(),
            None => Vec::new(),
        },
        _ => Vec::new(),
    }
}

fn die(msg: &str) -> ! {
    eprintln!("haj: {msg}");
    std::process::exit(1);
}
