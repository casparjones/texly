//! Opt-in dynamic font downloader.
//!
//! Before a compile, Texly can scan the document preamble for requested font
//! families (`\setmainfont` / `\setsansfont` / `\setmonofont` / `\newfontfamily`),
//! check whether each is already available to fontconfig, and fetch any missing
//! ones from a **whitelisted** source (the Google Fonts GitHub repo) into a
//! persistent on-disk cache that fontconfig scans. Disabled unless
//! `TEXLY_FONT_AUTODOWNLOAD=1`.
//!
//! ## Trust model
//! Texly is a single-tenant, authenticated, self-hosted editor — documents come
//! from the logged-in user, not anonymous input. The downloader is nonetheless
//! defensive: font names are strictly sanitized (letters/digits/spaces/hyphen
//! only), the download source is a hardcoded whitelist (no arbitrary URLs),
//! downloads are cached (no live fetch per request once cached), each network
//! call has a timeout, and per-family locks prevent concurrent-download races.

use std::{collections::BTreeSet, path::Path, sync::Arc};

use dashmap::DashMap;
use tokio::{process::Command, time::Duration};

use crate::AppState;

/// Hardcoded, whitelisted font source. We only ever talk to these hosts/paths.
const GH_API_CONTENTS: &str = "https://api.github.com/repos/google/fonts/contents";
/// Google Fonts groups families by license directory.
const LICENSE_DIRS: [&str; 3] = ["ofl", "apache", "ufl"];
/// Per-network-call timeout (seconds).
const WGET_TIMEOUT_SECS: u32 = 20;
/// Upper bound on the whole font-resolution step so it can never hang a compile.
const TOTAL_BUDGET_SECS: u64 = 60;

/// Per-family download locks, shared via [`AppState`], so two concurrent compiles
/// requesting the same missing family don't download it twice.
pub type FontLocks = Arc<DashMap<String, Arc<tokio::sync::Mutex<()>>>>;

/// Extract requested font families from LaTeX source.
///
/// Handles `\setmainfont`, `\setsansfont`, `\setmonofont` and `\newfontfamily`,
/// with optional `[options]` before or after the family argument, and the
/// `\newfontfamily\cmd{Family}` command-token form. TeX comments are ignored.
pub fn extract_requested_families(src: &str) -> Vec<String> {
    let src = strip_tex_comments(src);
    const COMMANDS: [&str; 4] = [
        "\\setmainfont",
        "\\setsansfont",
        "\\setmonofont",
        "\\newfontfamily",
    ];

    let bytes = src.as_bytes();
    let mut out: BTreeSet<String> = BTreeSet::new();

    for cmd in COMMANDS {
        let mut from = 0usize;
        while let Some(rel) = src[from..].find(cmd) {
            let mut i = from + rel + cmd.len();
            // The command must be followed by a non-letter (so "\setmainfontX"
            // doesn't match \setmainfont).
            let next_is_letter = bytes
                .get(i)
                .map(|b| b.is_ascii_alphabetic())
                .unwrap_or(false);
            if next_is_letter {
                from = from + rel + cmd.len();
                continue;
            }
            if let Some((family, end)) = parse_family_arg(&src, i) {
                if let Some(clean) = sanitize_family(&family) {
                    out.insert(clean);
                }
                i = end;
            }
            from = i.max(from + rel + cmd.len());
        }
    }

    out.into_iter().collect()
}

/// Starting just after a font command, skip an optional control-sequence token
/// (e.g. the `\cmd` of `\newfontfamily\cmd`), any `[...]` option blocks and
/// whitespace, then read the family from the first balanced `{...}` group.
/// Returns the family text and the byte index just past the closing brace.
fn parse_family_arg(src: &str, mut i: usize) -> Option<(String, usize)> {
    let b = src.as_bytes();
    let n = b.len();

    loop {
        // skip whitespace
        while i < n && (b[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= n {
            return None;
        }
        match b[i] as char {
            // control sequence like \cmd — skip backslash + following letters
            '\\' => {
                i += 1;
                while i < n && (b[i] as char).is_ascii_alphabetic() {
                    i += 1;
                }
            }
            // optional [..] block — skip to matching ]
            '[' => {
                let mut depth = 1;
                i += 1;
                while i < n && depth > 0 {
                    match b[i] as char {
                        '[' => depth += 1,
                        ']' => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
            }
            '{' => {
                // read balanced {...}
                let mut depth = 1;
                let start = i + 1;
                i += 1;
                while i < n && depth > 0 {
                    match b[i] as char {
                        '{' => depth += 1,
                        '}' => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
                if depth != 0 {
                    return None;
                }
                let family = src.get(start..i - 1)?.trim().to_string();
                return Some((family, i));
            }
            _ => return None,
        }
    }
}

/// Remove TeX line comments (unescaped `%` to end of line) so commented-out
/// font commands aren't picked up.
fn strip_tex_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    for line in src.split_inclusive('\n') {
        let bytes = line.as_bytes();
        let mut cut = line.len();
        let mut j = 0;
        while j < bytes.len() {
            if bytes[j] == b'%' && (j == 0 || bytes[j - 1] != b'\\') {
                cut = j;
                break;
            }
            j += 1;
        }
        out.push_str(&line[..cut]);
        if cut < line.len() && line.ends_with('\n') {
            out.push('\n');
        }
    }
    out
}

/// Accept only safe family names: ASCII letters, digits, spaces and hyphens,
/// trimmed, non-empty, at most 64 chars. Anything else (control sequences,
/// path characters, options that leaked in) is rejected.
pub fn sanitize_family(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 64 {
        return None;
    }
    if name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-')
    {
        Some(name.to_string())
    } else {
        None
    }
}

/// Google-Fonts-style directory slug: lowercase, alphanumerics only.
/// "Fira Sans" -> "firasans", "EB Garamond" -> "ebgaramond".
pub fn slug(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Is the family already known to fontconfig? Uses `fc-list "<family>"`, which
/// prints matching font files only when the family exists.
async fn family_present(name: &str) -> bool {
    match Command::new("fc-list")
        .arg(name)
        .arg("family")
        .output()
        .await
    {
        Ok(out) => !out.stdout.is_empty(),
        Err(_) => false, // no fontconfig → can't tell; treat as absent
    }
}

/// Resolve every requested family in `src`, downloading missing ones if enabled.
/// Best-effort: failures are logged, never propagated (a genuinely missing font
/// is then handled by fontspec / the `texly-fonts.tex` safety net).
pub async fn ensure_fonts_for_source(state: &AppState, src: &str) {
    if !state.font_autodownload {
        return;
    }
    let families = extract_requested_families(src);
    if families.is_empty() {
        return;
    }

    let _ = tokio::time::timeout(Duration::from_secs(TOTAL_BUDGET_SECS), async {
        for family in families {
            if family_present(&family).await {
                continue;
            }
            if let Err(e) = resolve_family(state, &family).await {
                tracing::warn!("font auto-download for {family:?} failed: {e}");
            }
        }
    })
    .await;
}

/// Acquire the per-family lock, re-check the cache, then download.
async fn resolve_family(state: &AppState, family: &str) -> anyhow::Result<()> {
    let slug = slug(family);
    if slug.is_empty() {
        return Ok(());
    }

    let lock = state
        .font_locks
        .entry(slug.clone())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone();
    let _guard = lock.lock().await;

    // Another compile may have fetched it while we waited for the lock.
    if family_present(family).await {
        return Ok(());
    }

    let target = state.font_cache_dir.join(&slug);
    if target.is_dir() && dir_has_fonts(&target) {
        // Cached on disk but fontconfig hasn't indexed it yet — just rebuild.
        run_fc_cache(&state.font_cache_dir).await;
        return Ok(());
    }

    download_family(family, &slug, &state.font_cache_dir).await?;
    run_fc_cache(&state.font_cache_dir).await;
    Ok(())
}

fn dir_has_fonts(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok()).any(|e| {
                let p = e.path();
                matches!(
                    p.extension().and_then(|x| x.to_str()),
                    Some("ttf") | Some("otf")
                )
            })
        })
        .unwrap_or(false)
}

/// Download a family from the whitelisted Google Fonts repo into
/// `cache_dir/<slug>/`. Tries each license directory until one matches.
async fn download_family(family: &str, slug: &str, cache_dir: &Path) -> anyhow::Result<()> {
    for license in LICENSE_DIRS {
        // slug is alphanumeric-only, so it cannot escape the URL path.
        let list_url = format!("{GH_API_CONTENTS}/{license}/{slug}");
        let listing = match wget_bytes(&list_url).await {
            Ok(bytes) => bytes,
            Err(_) => continue, // 404 / network → try next license dir
        };
        let json: serde_json::Value = match serde_json::from_slice(&listing) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let entries = match json.as_array() {
            Some(a) => a,
            None => continue, // e.g. {"message":"Not Found"}
        };

        let files: Vec<(&str, &str)> = entries
            .iter()
            .filter_map(|e| {
                let name = e.get("name")?.as_str()?;
                let url = e.get("download_url")?.as_str()?;
                let lower = name.to_ascii_lowercase();
                if lower.ends_with(".ttf") || lower.ends_with(".otf") {
                    Some((name, url))
                } else {
                    None
                }
            })
            .collect();

        if files.is_empty() {
            continue;
        }

        // Download into a temp dir, then atomically rename into place.
        let tmp = cache_dir.join(format!(".{slug}.tmp"));
        let _ = tokio::fs::remove_dir_all(&tmp).await;
        tokio::fs::create_dir_all(&tmp).await?;

        let mut got = 0usize;
        for (name, url) in &files {
            // download_url is from the whitelisted repo listing; restrict to the
            // raw host as belt-and-suspenders.
            if !url.starts_with("https://raw.githubusercontent.com/google/fonts/") {
                continue;
            }
            // file name from the listing — keep only the basename, no separators.
            let fname = Path::new(name)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if fname.is_empty() || fname.contains('/') {
                continue;
            }
            if wget_to_file(url, &tmp.join(fname)).await.is_ok() {
                got += 1;
            }
        }

        if got == 0 {
            let _ = tokio::fs::remove_dir_all(&tmp).await;
            continue;
        }

        let target = cache_dir.join(slug);
        let _ = tokio::fs::remove_dir_all(&target).await;
        tokio::fs::rename(&tmp, &target).await?;
        tracing::info!("downloaded {got} font file(s) for {family:?} from {license}/{slug}");
        return Ok(());
    }

    anyhow::bail!("family not found in whitelisted source")
}

/// Fetch a URL into memory via `wget` (already in the runtime image; avoids an
/// HTTP-client dependency and matches how Tectonic is installed).
async fn wget_bytes(url: &str) -> anyhow::Result<Vec<u8>> {
    let out = Command::new("wget")
        .arg("-qO-")
        .arg("--header=User-Agent: texly")
        .arg(format!("--timeout={WGET_TIMEOUT_SECS}"))
        .arg("--tries=1")
        .arg(url)
        .output()
        .await?;
    if !out.status.success() {
        anyhow::bail!("wget failed for {url}");
    }
    Ok(out.stdout)
}

async fn wget_to_file(url: &str, dest: &Path) -> anyhow::Result<()> {
    let out = Command::new("wget")
        .arg("-qO")
        .arg(dest)
        .arg("--header=User-Agent: texly")
        .arg(format!("--timeout={WGET_TIMEOUT_SECS}"))
        .arg("--tries=2")
        .arg(url)
        .output()
        .await?;
    if !out.status.success() {
        let _ = tokio::fs::remove_file(dest).await;
        anyhow::bail!("wget failed for {url}");
    }
    Ok(())
}

async fn run_fc_cache(dir: &Path) {
    let _ = Command::new("fc-cache").arg("-f").arg(dir).output().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_basic_commands() {
        let src = r#"
            \setmainfont{EB Garamond 12}
            \setsansfont[Scale=0.9]{Fira Sans}
            \setmonofont{JetBrains Mono}[Scale=MatchLowercase]
            \newfontfamily\heading{TeX Gyre Heros}
        "#;
        let fams = extract_requested_families(src);
        assert!(fams.contains(&"EB Garamond 12".to_string()));
        assert!(fams.contains(&"Fira Sans".to_string()));
        assert!(fams.contains(&"JetBrains Mono".to_string()));
        assert!(fams.contains(&"TeX Gyre Heros".to_string()));
    }

    #[test]
    fn ignores_comments_and_non_font_commands() {
        let src =
            "% \\setmainfont{Should Not Appear}\n\\setmainfont{Real Font}\n\\setmainfontsize{12}";
        let fams = extract_requested_families(src);
        assert_eq!(fams, vec!["Real Font".to_string()]);
    }

    #[test]
    fn sanitize_rejects_injection() {
        assert!(sanitize_family("Fira Sans").is_some());
        assert!(sanitize_family("EB Garamond 12").is_some());
        assert!(sanitize_family("../../etc/passwd").is_none());
        assert!(sanitize_family("Foo$(rm -rf)").is_none());
        assert!(sanitize_family("a\\cmd").is_none());
        assert!(sanitize_family("   ").is_none());
        assert!(sanitize_family(&"x".repeat(100)).is_none());
    }

    #[test]
    fn slug_matches_google_convention() {
        assert_eq!(slug("Fira Sans"), "firasans");
        assert_eq!(slug("EB Garamond"), "ebgaramond");
        assert_eq!(slug("JetBrains Mono"), "jetbrainsmono");
    }
}
