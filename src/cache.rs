//! 一行説明(`--haj-describe`)のキャッシュ。
//!
//! `haj help` とTAB補完は、コマンドの数だけサブプロセスを起動して説明を聞く。
//! bashのサブコマンド1本で実測 9ms 程度かかるので、20本あればTABのたびに
//! 200ms 近く待たされることになる。これは体感ではっきり遅い。
//!
//! 説明文はコマンドのファイルが変わらない限り変わらないので、
//! (パス, 更新時刻, サイズ) を鍵にキャッシュする。触っていないコマンドは
//! 二度と起動されない。

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

pub struct DescribeCache {
    /// stamp(パス+mtime+サイズ) → 説明文
    entries: HashMap<String, String>,
    dirty: bool,
    path: Option<PathBuf>,
}

impl DescribeCache {
    pub fn load() -> Self {
        let path = cache_file();
        let mut entries = HashMap::new();

        if let Some(p) = &path {
            if let Ok(content) = fs::read_to_string(p) {
                for line in content.lines() {
                    // 形式: <パス>\t<mtime>\t<サイズ>\t<説明>
                    // 説明にタブは入らない前提(入っていたら捨てる = 次回聞き直す)
                    let cols: Vec<&str> = line.split('\t').collect();
                    if cols.len() == 4 {
                        let stamp = cols[..3].join("\t");
                        entries.insert(stamp, cols[3].to_string());
                    }
                }
            }
        }

        Self {
            entries,
            dirty: false,
            path,
        }
    }

    /// キャッシュにあればそれを、無ければ `fetch` を呼んで覚える。
    pub fn get_or_insert<F>(&mut self, stamp: Option<String>, fetch: F) -> Option<String>
    where
        F: FnOnce() -> Option<String>,
    {
        // stampが取れない(ファイルが消えた等)ならキャッシュを介さず素で聞く
        let Some(stamp) = stamp else { return fetch() };

        if let Some(hit) = self.entries.get(&stamp) {
            return Some(hit.clone());
        }

        let value = fetch()?;
        // 説明にタブや改行が混ざるとキャッシュの行形式が壊れるので、ここで潰す。
        let value = value.replace(['\t', '\n', '\r'], " ").trim().to_string();
        self.entries.insert(stamp, value.clone());
        self.dirty = true;
        Some(value)
    }

    /// 変更があれば書き出す。失敗しても何も言わない(キャッシュは無くても動く)。
    ///
    /// 一時ファイルへ書いてから rename する。補完は同時に何本も走りうるので、
    /// 直接上書きすると読み手が壊れた行を読むことがある。
    pub fn save(&self) {
        if !self.dirty {
            return;
        }
        let Some(path) = &self.path else { return };
        let Some(dir) = path.parent() else { return };
        if fs::create_dir_all(dir).is_err() {
            return;
        }

        let tmp = dir.join(format!("describe.{}.tmp", std::process::id()));
        let ok = (|| -> std::io::Result<()> {
            let mut f = fs::File::create(&tmp)?;
            for (stamp, desc) in &self.entries {
                writeln!(f, "{stamp}\t{desc}")?;
            }
            f.sync_all()
        })()
        .is_ok();

        if ok {
            let _ = fs::rename(&tmp, path);
        } else {
            let _ = fs::remove_file(&tmp);
        }
    }
}

/// キャッシュの置き場所。XDG に従う。HAJ_NO_CACHE=1 で無効化できる(デバッグ用)。
fn cache_file() -> Option<PathBuf> {
    if std::env::var("HAJ_NO_CACHE").is_ok_and(|v| v == "1") {
        return None;
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    Some(base.join("haj").join("describe.tsv"))
}
