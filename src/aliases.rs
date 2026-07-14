//! エイリアスの解決(SPEC §2.7)。
//!
//! 定義できる場所は2つで、近いスコープが勝つ:
//!
//!   1. プロジェクトの `.haj/config`(カレントから遡って近い順)
//!   2. ユーザー設定 `~/.config/haj/config`
//!
//! プロジェクト側に書けるのは package.json の scripts に相当する「1行の委譲」の
//! ため(Issue #11)。ロジックを持つタスクは従来どおり `.haj/commands/` に置く
//! (§11 — タスクランナーは作らない)。1行で書けなくなったら commands/ へ昇格する。
//!
//! 「clone しただけでエイリアスを仕込める」危険は `.haj/commands/` が既に持つ露出と
//! 同じで、エイリアス固有の問題ではない。守るべき線は「このツリーを信頼するか」で
//! あり、ツリー全体の信頼ゲート(Issue #1)で扱う。

use crate::project::Origin;

/// 解決済みのエイリアス1つ。どこ由来かを常に持ち歩く(素性の可視化)。
pub struct Alias {
    pub name: String,
    pub expansion: String,
    pub desc: Option<String>,
    pub origin: Origin,
}

impl Alias {
    /// 一覧・補完に出す説明。`.desc` があればそれ、無ければ展開そのもの
    /// (長いものは切り詰める。読めないより短い方がまし)。
    pub fn summary(&self) -> String {
        if let Some(d) = &self.desc {
            return d.clone();
        }
        const MAX: usize = 48;
        let one_line: String = self
            .expansion
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        if one_line.chars().count() <= MAX {
            format!("→ haj {one_line}")
        } else {
            let short: String = one_line.chars().take(MAX).collect();
            format!("→ haj {short}…")
        }
    }
}

/// プロジェクト・スコープの定義(`.haj/config` の alias.*)を近い順に返す。
fn project_scopes() -> Vec<(std::collections::HashMap<String, String>, Origin)> {
    let mut scopes = Vec::new();
    for (tree, origin) in crate::discovery::project_trees() {
        if let Ok(s) = std::fs::read_to_string(tree.join("config")) {
            scopes.push((crate::config::parse_kv(&s), origin));
        }
    }
    scopes
}

/// 名前から展開を引く。プロジェクト(近い順) > ユーザー設定。
/// 予約語は引かない(予約語 > エイリアス > 探索、の順は誰にも変えさせない)。
pub fn lookup(name: &str) -> Option<Alias> {
    if crate::discovery::is_reserved(name) {
        return None;
    }
    let key = format!("alias.{name}");
    for (map, origin) in project_scopes() {
        if let Some(v) = map.get(&key).filter(|v| !v.is_empty()) {
            return Some(Alias {
                name: name.to_string(),
                expansion: v.clone(),
                desc: desc_of(&map, name),
                origin,
            });
        }
    }
    let cfg = crate::config::Config::load();
    cfg.alias(name).map(|expansion| Alias {
        name: name.to_string(),
        desc: cfg.alias_desc(name),
        expansion,
        origin: Origin::User,
    })
}

/// 呼べるエイリアスを全部返す(名前順)。同名は近いスコープが勝つ。
pub fn list() -> Vec<Alias> {
    let mut out: Vec<Alias> = Vec::new();

    for (map, origin) in project_scopes() {
        for (name, v) in aliases_in(&map) {
            push_unless_shadowed(
                &mut out,
                Alias {
                    desc: desc_of(&map, &name),
                    name,
                    expansion: v,
                    origin: origin.clone(),
                },
            );
        }
    }

    let cfg = crate::config::Config::load();
    for (name, expansion) in cfg.aliases() {
        push_unless_shadowed(
            &mut out,
            Alias {
                desc: cfg.alias_desc(&name),
                name,
                expansion,
                origin: Origin::User,
            },
        );
    }

    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// map から `alias.<名前> = <展開>` を抜く。規則は config::Config::aliases と同じ
/// (`.desc` は説明であってエイリアスではない。予約語と空値は無視)。
fn aliases_in(map: &std::collections::HashMap<String, String>) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = map
        .iter()
        .filter_map(|(k, val)| {
            let name = k.strip_prefix("alias.")?;
            (!name.is_empty()
                && !name.ends_with(".desc")
                && !val.is_empty()
                && !crate::discovery::is_reserved(name))
            .then(|| (name.to_string(), val.clone()))
        })
        .collect();
    v.sort();
    v
}

fn desc_of(map: &std::collections::HashMap<String, String>, name: &str) -> Option<String> {
    map.get(&format!("alias.{name}.desc"))
        .filter(|v| !v.is_empty())
        .cloned()
}

fn push_unless_shadowed(out: &mut Vec<Alias>, a: Alias) {
    if !out.iter().any(|e| e.name == a.name) {
        out.push(a);
    }
}
