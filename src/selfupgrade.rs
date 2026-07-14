//! `haj selfupgrade` — コア自身をリリース版で置き換える(SPEC.md §9.1)。
//!
//! install.sh と同じ経路・同じ環境変数を使う。チェックアウト無しで
//! 「入っている haj を更新する」ためのもの。
//!
//! **依存ゼロとの折り合い**: stdlib に HTTP は無いが crate は増やさない。
//! 取得・展開・照合は curl / tar / sha256sum を子プロセスとして呼ぶ。
//! exec がコアの本業、という設計に沿っている。

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 公開プロファイルでは空 = 取得元が設定されるまで selfupgrade は動かない
/// (GitHub Releases 対応は #10 フェーズ3)。
pub const DEFAULT_GITLAB: &str = crate::profile::pick("https://gitlab.avaper.day", "");
pub const DEFAULT_PROJECT_ID: &str = crate::profile::pick("788", "");
pub const DEFAULT_TARGET: &str = "x86_64-unknown-linux-musl";

struct Config {
    gitlab: String,
    project_id: String,
    target: String,
    token: String,
}

impl Config {
    fn load() -> Result<Self, String> {
        let cfg = crate::config::Config::load();

        let token = cfg
            .get_opt("HAJ_TOKEN", "selfupgrade.token")
            .map(|(v, _)| v)
            .ok_or_else(|| {
                let where_to = cfg
                    .path
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "~/.config/haj/config".to_string());
                format!(
                    "GitLabのトークンがありません。このリポジトリはprivateなので、\
                     リリースの取得に必要です。\n  \
                     環境変数 HAJ_TOKEN を渡すか、{where_to} に書いてください:\n    \
                     selfupgrade.token = glpat-xxxxxxxx\n    \
                     selfupgrade.token = vault://users/<名前>/gitlab-pat/gitlab.avaper.day/token  (参照でもよい)"
                )
            })?;

        // token にはシークレット参照を書ける(SPEC §8.4)。平文 PAT をディスクに
        // 置かずに済ませるため、使うこの瞬間に展開する。~/.config/haj/config は
        // 本人しか書けないファイルなので、参照を書いたこと自体が同意 — ゲート不要。
        let token = crate::secrets::expand(&token, false)
            .map_err(|e| format!("token: {e}"))?
            .unwrap_or(token);

        Ok(Config {
            gitlab: cfg
                .get("HAJ_GITLAB", "selfupgrade.gitlab", DEFAULT_GITLAB)
                .0,
            project_id: cfg
                .get(
                    "HAJ_PROJECT_ID",
                    "selfupgrade.project_id",
                    DEFAULT_PROJECT_ID,
                )
                .0,
            target: cfg
                .get("HAJ_TARGET", "selfupgrade.target", DEFAULT_TARGET)
                .0,
            token,
        })
    }

    /// CI_JOB_TOKEN と同値なら JOB-TOKEN ヘッダ、それ以外は PRIVATE-TOKEN ヘッダ。
    fn auth_header(&self) -> String {
        match std::env::var("CI_JOB_TOKEN") {
            Ok(t) if t == self.token => format!("JOB-TOKEN: {}", self.token),
            _ => format!("PRIVATE-TOKEN: {}", self.token),
        }
    }

    fn package_url(&self, version: &str, file: &str) -> String {
        format!(
            "{}/api/v4/projects/{}/packages/generic/haj/{}/{}",
            self.gitlab, self.project_id, version, file
        )
    }
}

pub fn run(args: &[String]) -> ! {
    let check_only = args.iter().any(|a| a == "--check");
    let wanted: Option<&str> = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .map(String::as_str);

    match main(wanted, check_only) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("haj selfupgrade: {e}");
            // --check は「調べられなかった」を 2 で返す(SPEC §9.1)
            std::process::exit(if check_only { 2 } else { 1 });
        }
    }
}

fn main(wanted: Option<&str>, check_only: bool) -> Result<i32, String> {
    require("curl")?;

    let cfg = Config::load()?;

    // 版が未指定なら最新を調べる。指定されていれば無条件にそれを入れる
    // (同版の再インストールもダウングレードもこれで表現する。--force は設けない)。
    let version = match wanted {
        Some(v) => v.trim_start_matches('v').to_string(),
        None => latest_version(&cfg)?,
    };

    if check_only {
        // 版の比較は文字列の一致のみ。semver は解釈しない。
        if version == VERSION {
            println!("haj {VERSION} は最新です");
            return Ok(0);
        }
        println!("更新があります: {VERSION} → {version}");
        return Ok(1);
    }

    if wanted.is_none() && version == VERSION {
        println!("haj {VERSION} は最新です。何もしません。");
        return Ok(0);
    }

    let current = std::env::current_exe()
        .map_err(|e| format!("自分自身のパスが分かりません: {e}"))?
        // 実体を辿る。シンボリックリンク越しに置き換えるとリンクを壊す。
        .canonicalize()
        .map_err(|e| format!("自分自身のパスを解決できません: {e}"))?;

    // 置き換え先に書けるかを、ダウンロードより先に確かめる。
    // 落としてから「書けません」では時間の無駄だし、混乱する。
    let dir = current
        .parent()
        .ok_or_else(|| format!("{} の親ディレクトリが分かりません", current.display()))?;
    if !writable(dir) {
        return Err(format!(
            "{} に書けません。sudo で実行し直してください:\n  \
             sudo -E haj selfupgrade{}",
            dir.display(),
            wanted.map(|v| format!(" {v}")).unwrap_or_default()
        ));
    }

    println!(
        "==> haj {VERSION} → {version} ({}) を取得します",
        cfg.target
    );

    let work = TempDir::new()?;
    let archive = format!("haj-{}.tar.gz", cfg.target);
    let archive_path = work.path.join(&archive);

    download(&cfg, &cfg.package_url(&version, &archive), &archive_path).map_err(|e| {
        format!(
            "{e}\n  版 {version} が存在しないか、トークンに read_api / \
             read_package_registry の権限がありません。"
        )
    })?;

    verify_checksum(&cfg, &version, &archive, &work.path)?;

    // 展開
    let ok = Command::new("tar")
        .args(["xzf", &archive_path.to_string_lossy(), "-C"])
        .arg(&work.path)
        .status()
        .map_err(|e| format!("tar を実行できません: {e}"))?
        .success();
    if !ok {
        return Err("アーカイブを展開できませんでした".into());
    }

    let new_bin = work.path.join("haj");
    if !new_bin.is_file() {
        return Err("アーカイブに haj が入っていません".into());
    }
    fs::set_permissions(&new_bin, fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("実行ビットを立てられません: {e}"))?;

    // 健全性チェック: 落としたバイナリが本当に動くか。壊れたもので自分を潰さない。
    let out = Command::new(&new_bin)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("落としたバイナリを実行できません: {e}"))?;
    let banner = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() || !banner.starts_with("haj ") {
        return Err(format!(
            "落としたバイナリが正しく動きません(--version の出力: {:?})",
            banner.trim()
        ));
    }

    // 現バイナリと**同じディレクトリ**に置いてから rename する。
    // 同一ファイルシステム内なので原子的に置き換わり、実行中のプロセス
    // (いま動いているこの haj 自身を含む)には影響しない。
    // /tmp から直接 rename すると、ファイルシステムを跨いで EXDEV で失敗する。
    let staged = dir.join(format!(".haj.selfupgrade.{}", std::process::id()));
    fs::copy(&new_bin, &staged).map_err(|e| format!("{} に置けません: {e}", staged.display()))?;
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("実行ビットを立てられません: {e}"))?;

    if let Err(e) = fs::rename(&staged, &current) {
        let _ = fs::remove_file(&staged);
        return Err(format!("{} を置き換えられません: {e}", current.display()));
    }

    println!(
        "==> {} を {} に更新しました",
        current.display(),
        banner.trim()
    );
    Ok(0)
}

/// releases API の先頭から tag_name を取る。
///
/// JSONパーサは持ち込まない。欲しいのは最初の "tag_name":"..." ひとつだけで、
/// そのために serde を足すのは割に合わない。
fn latest_version(cfg: &Config) -> Result<String, String> {
    let url = format!(
        "{}/api/v4/projects/{}/releases?per_page=1",
        cfg.gitlab, cfg.project_id
    );
    let out = Command::new("curl")
        .args(["-fsSL", "--header", &cfg.auth_header(), &url])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("curl を実行できません: {e}"))?;

    if !out.status.success() {
        return Err(format!(
            "リリース一覧を取得できません ({}). トークンとネットワークを確認してください。",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    let body = String::from_utf8_lossy(&out.stdout);
    let tag = body
        .split("\"tag_name\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .ok_or("リリースが見つかりません。版を明示してください: haj selfupgrade 0.1.0")?;

    Ok(tag.trim_start_matches('v').to_string())
}

fn download(cfg: &Config, url: &str, dest: &Path) -> Result<(), String> {
    let out = Command::new("curl")
        .args(["-fsSL", "--header", &cfg.auth_header(), "-o"])
        .arg(dest)
        .arg(url)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("curl を実行できません: {e}"))?;
    if !out.status.success() {
        return Err(format!("取得に失敗しました: {url}"));
    }
    Ok(())
}

/// `.sha256` が取れたら照合する。不一致は中止。`.sha256` 自体が無ければ省略する
/// (install.sh と同じ振る舞い)。
fn verify_checksum(cfg: &Config, version: &str, archive: &str, work: &Path) -> Result<(), String> {
    let sums = format!("{archive}.sha256");
    let url = cfg.package_url(version, &sums);
    if download(cfg, &url, &work.join(&sums)).is_err() {
        eprintln!("  (チェックサムが公開されていません。照合を省略します)");
        return Ok(());
    }
    if which("sha256sum").is_none() {
        eprintln!("  (sha256sum がありません。照合を省略します)");
        return Ok(());
    }

    let ok = Command::new("sha256sum")
        .arg("-c")
        .arg(&sums)
        .current_dir(work)
        .stdout(Stdio::null())
        .status()
        .map_err(|e| format!("sha256sum を実行できません: {e}"))?
        .success();

    if !ok {
        return Err("チェックサムが一致しません。取得し直してください。".into());
    }
    println!("==> チェックサム OK");
    Ok(())
}

fn require(exe: &str) -> Result<(), String> {
    which(exe)
        .map(|_| ())
        .ok_or_else(|| format!("{exe} が必要です。入れてから実行してください。"))
}

fn which(exe: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .to_str()?
        .split(':')
        .map(|d| Path::new(d).join(exe))
        .find(|p| p.is_file())
}

/// 実際に書けるか。パーミッションのビットを読むより、書いてみるほうが確実
/// (root、ACL、読み取り専用マウントなどを一発で判定できる)。
fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".haj.wtest.{}", std::process::id()));
    match fs::File::create(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// 後始末付きの作業ディレクトリ。
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Result<Self, String> {
        let base = std::env::temp_dir().join(format!("haj-selfupgrade-{}", std::process::id()));
        fs::create_dir_all(&base).map_err(|e| format!("作業ディレクトリを作れません: {e}"))?;
        Ok(Self { path: base })
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

/// `haj help selfupgrade` 用。コア組み込みなので --haj-help は持てない。
pub fn long_help() -> String {
    format!(
        r#"haj selfupgrade [<版>] [--check] — コア自身を更新する

  haj selfupgrade            最新リリースを調べ、今と違えば置き換える
  haj selfupgrade 0.2.0      その版を無条件に入れる(再インストール/ダウングレードもこれ)
  haj selfupgrade --check    調べるだけ。終了コード: 0 最新 / 1 更新あり / 2 調べられず

現在: haj {VERSION}

設定は ~/.config/haj/config に書ける(環境変数でも渡せる。haj config で実効値を確認)。

  設定ファイルの鍵          環境変数           既定値
  selfupgrade.token       HAJ_TOKEN         (必須。vault:// などの参照でもよい)
  selfupgrade.gitlab      HAJ_GITLAB        https://gitlab.avaper.day
  selfupgrade.project_id  HAJ_PROJECT_ID    788
  selfupgrade.target      HAJ_TARGET        x86_64-unknown-linux-musl

置き換えは、現バイナリと同じディレクトリに書いてから rename する(原子的で、
実行中のプロセスに影響しない)。書けない場所なら sudo での再実行を提案して終わる。
コア自身は昇格しない。"#
    )
}
