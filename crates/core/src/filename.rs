//! Choosing a safe, unique file name for a download (SPEC-engine §4).

use std::path::{Path, PathBuf};

use reqwest::Url;

/// Longest name we produce, in bytes. Linux allows 255 bytes per name and we add
/// `.slipstream.part` (16 bytes) while downloading, so leave room for that.
const MAX_LEN: usize = 230;

/// Suffixes that belong together when we add " (2)" to a clashing name.
const DOUBLE_EXTENSIONS: [&str; 4] = [".tar.gz", ".tar.xz", ".tar.bz2", ".tar.zst"];

/// The name to save a download under, from the most to the least trusted source:
/// `Content-Disposition` → `?filename=`/`?file=`/`?name=` → last path segment →
/// `download`. Adds a missing extension from the MIME type and sanitizes the result.
pub fn choose(content_disposition: Option<&str>, url: &Url, mime: Option<&str>) -> String {
    let raw = content_disposition
        .and_then(from_content_disposition)
        .or_else(|| from_url(url))
        .unwrap_or_default();
    let mut name = sanitize(&raw);
    if !has_extension(&name)
        && let Some(ext) = mime.and_then(extension_for_mime)
    {
        name = sanitize(&format!("{name}.{ext}"));
    }
    name
}

/// `filename*` (RFC 5987/6266, may be UTF-8) wins over plain `filename`.
pub fn from_content_disposition(value: &str) -> Option<String> {
    let mut plain = None;
    for param in split_params(value) {
        let Some((key, val)) = param.split_once('=') else {
            continue;
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "filename*" => {
                if let Some(name) = decode_ext_value(unquote(val.trim()).as_str()) {
                    return Some(name).filter(|n| !n.is_empty());
                }
            }
            "filename" => plain = Some(unquote(val.trim())),
            _ => {}
        }
    }
    plain.filter(|n| !n.is_empty())
}

/// A name from the URL: common query parameters first, then the last path segment.
pub fn from_url(url: &Url) -> Option<String> {
    let from_query = url
        .query_pairs()
        .find(|(k, v)| matches!(k.as_ref(), "filename" | "file" | "name") && !v.is_empty())
        .map(|(_, v)| v.into_owned());
    from_query.or_else(|| {
        let last = url.path_segments()?.rfind(|s| !s.is_empty())?;
        Some(String::from_utf8_lossy(&pct_decode(last)).into_owned())
    })
}

/// Make `name` valid on both Windows and Linux.
pub fn sanitize(name: &str) -> String {
    // Never let a name climb out of the download folder ("../../x").
    let base = name.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = base
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    // Windows silently drops trailing dots and spaces, so do it ourselves.
    let mut name = cleaned.trim_start().trim_end_matches(['.', ' ']).to_owned();
    if name.is_empty() {
        return "download".to_owned();
    }
    let stem = name.split('.').next().unwrap_or("").trim_end();
    if is_reserved_on_windows(stem) {
        name.insert(0, '_');
    }
    truncate(&name, MAX_LEN)
}

/// `dir/name` if nothing is in the way, otherwise `name (2).ext`, `name (3).ext`...
/// `taken` says whether a candidate path is already used (on disk or in the list).
pub fn unique(dir: &Path, name: &str, taken: impl Fn(&Path) -> bool) -> PathBuf {
    let (stem, ext) = split_extension(name);
    (1..)
        .map(|n| match n {
            1 => dir.join(name),
            n => dir.join(format!("{stem} ({n}){ext}")),
        })
        .find(|p| !taken(p))
        .unwrap()
}

/// File extension for common download MIME types (only used when the name has none).
fn extension_for_mime(mime: &str) -> Option<&'static str> {
    let essence = mime.split(';').next()?.trim().to_ascii_lowercase();
    Some(match essence.as_str() {
        "application/zip" | "application/x-zip-compressed" => "zip",
        "application/pdf" => "pdf",
        "application/gzip" | "application/x-gzip" => "gz",
        "application/x-7z-compressed" => "7z",
        "application/vnd.rar" | "application/x-rar-compressed" => "rar",
        "application/x-tar" => "tar",
        "application/x-xz" => "xz",
        "application/x-msdownload" | "application/vnd.microsoft.portable-executable" => "exe",
        "application/x-msi" => "msi",
        "application/vnd.android.package-archive" => "apk",
        "application/vnd.debian.binary-package" | "application/x-debian-package" => "deb",
        "application/x-rpm" => "rpm",
        "application/x-iso9660-image" => "iso",
        "application/epub+zip" => "epub",
        "application/json" => "json",
        "text/plain" => "txt",
        "text/csv" => "csv",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "audio/mpeg" => "mp3",
        "audio/flac" => "flac",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/mp4" => "m4a",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/x-matroska" => "mkv",
        "video/quicktime" => "mov",
        _ => return None,
    })
}

fn has_extension(name: &str) -> bool {
    matches!(name.rsplit_once('.'), Some((stem, ext)) if !stem.is_empty() && !ext.is_empty())
}

/// `("archive", ".tar.gz")`, `("photo", ".jpg")`, `(".bashrc", "")`.
fn split_extension(name: &str) -> (&str, &str) {
    let lower = name.to_ascii_lowercase();
    if let Some(ext) = DOUBLE_EXTENSIONS.iter().find(|e| lower.ends_with(*e))
        && name.len() > ext.len()
    {
        return name.split_at(name.len() - ext.len());
    }
    match name.rfind('.') {
        Some(i) if i > 0 => name.split_at(i),
        _ => (name, ""),
    }
}

fn is_reserved_on_windows(stem: &str) -> bool {
    let upper = stem.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper.len() == 4
            && upper.as_bytes()[3].is_ascii_digit()
            && upper.as_bytes()[3] != b'0')
}

/// Cut the stem so the whole name fits in `max` bytes, keeping the extension.
fn truncate(name: &str, max: usize) -> String {
    if name.len() <= max {
        return name.to_owned();
    }
    let (stem, ext) = split_extension(name);
    let ext = if ext.len() < max / 2 { ext } else { "" };
    let mut end = max - ext.len();
    while !stem.is_char_boundary(end.min(stem.len())) {
        end -= 1;
    }
    format!("{}{ext}", &stem[..end.min(stem.len())])
}

/// Split header parameters on `;`, ignoring `;` inside quoted strings.
fn split_params(value: &str) -> Vec<&str> {
    let (mut parts, mut start, mut quoted, mut escaped) = (Vec::new(), 0, false, false);
    for (i, c) in value.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ';' if !quoted => {
                parts.push(&value[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&value[start..]);
    parts
}

fn unquote(value: &str) -> String {
    match value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        Some(inner) => {
            let mut out = String::with_capacity(inner.len());
            let mut chars = inner.chars();
            while let Some(c) = chars.next() {
                out.push(if c == '\\' {
                    chars.next().unwrap_or('\\')
                } else {
                    c
                });
            }
            out
        }
        None => value.to_owned(),
    }
}

/// `UTF-8''%E2%82%AC.pdf` → `€.pdf` (RFC 5987 ext-value).
fn decode_ext_value(value: &str) -> Option<String> {
    let mut parts = value.splitn(3, '\'');
    let charset = parts.next()?.to_ascii_lowercase();
    let _language = parts.next()?;
    let bytes = pct_decode(parts.next()?);
    match charset.as_str() {
        "utf-8" => String::from_utf8(bytes).ok(),
        "iso-8859-1" => Some(bytes.iter().map(|&b| char::from(b)).collect()),
        _ => None,
    }
}

fn pct_decode(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn content_disposition() {
        let cd = |v| from_content_disposition(v);
        assert_eq!(
            cd("attachment; filename=\"a b.zip\""),
            Some("a b.zip".into())
        );
        assert_eq!(
            cd("attachment; filename=plain.txt"),
            Some("plain.txt".into())
        );
        assert_eq!(
            cd(
                "attachment; filename=\"fallback.pdf\"; filename*=UTF-8''%E2%82%AC%20r%C3%A9sum%C3%A9.pdf"
            ),
            Some("€ résumé.pdf".into())
        );
        assert_eq!(
            cd("attachment; filename*=iso-8859-1'en'%E9t%E9.txt"),
            Some("été.txt".into())
        );
        assert_eq!(
            cd("attachment; filename=\"semi;colon.txt\""),
            Some("semi;colon.txt".into())
        );
        assert_eq!(cd("inline"), None);
    }

    #[test]
    fn names_from_url() {
        assert_eq!(
            from_url(&url("https://x.org/dl/file%20one.iso")),
            Some("file one.iso".into())
        );
        assert_eq!(
            from_url(&url("https://x.org/get?id=3&filename=r.zip")),
            Some("r.zip".into())
        );
        assert_eq!(from_url(&url("https://x.org/dir/")), Some("dir".into()));
        assert_eq!(from_url(&url("https://x.org/")), None);
    }

    #[test]
    fn choose_falls_back_and_fixes_extension() {
        let u = url("https://x.org/download?id=7");
        assert_eq!(choose(None, &u, Some("application/zip")), "download.zip");
        assert_eq!(
            choose(Some("attachment"), &url("https://x.org/"), None),
            "download"
        );
        assert_eq!(
            choose(
                None,
                &url("https://x.org/a.tar.gz"),
                Some("application/gzip")
            ),
            "a.tar.gz"
        );
    }

    #[test]
    fn sanitizing() {
        assert_eq!(sanitize("../../etc/passwd"), "passwd");
        assert_eq!(sanitize("..\\..\\boot.ini"), "boot.ini");
        assert_eq!(sanitize("what?<is>:this*.txt"), "what__is__this_.txt");
        assert_eq!(sanitize("trailing. . "), "trailing");
        assert_eq!(sanitize("CON.txt"), "_CON.txt");
        assert_eq!(sanitize("com1"), "_com1");
        assert_eq!(sanitize("COM0.txt"), "COM0.txt");
        assert_eq!(sanitize("..."), "download");
        let long = format!("{}.zip", "é".repeat(200));
        let cut = sanitize(&long);
        assert!(cut.len() <= MAX_LEN && cut.ends_with(".zip"), "{cut}");
    }

    #[test]
    fn unique_names() {
        let dir = Path::new("d");
        let taken = [
            dir.join("a.zip"),
            dir.join("a (2).zip"),
            dir.join("x.tar.gz"),
        ];
        let is_taken = |p: &Path| taken.iter().any(|t| t == p);
        assert_eq!(unique(dir, "b.zip", is_taken), dir.join("b.zip"));
        assert_eq!(unique(dir, "a.zip", is_taken), dir.join("a (3).zip"));
        assert_eq!(unique(dir, "x.tar.gz", is_taken), dir.join("x (2).tar.gz"));
    }
}
