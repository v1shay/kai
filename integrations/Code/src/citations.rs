//! Classify link targets without invoking a shell or accessing the filesystem.
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileCitation {
    pub path: PathBuf,
    pub line: Option<u64>,
    pub end_line: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Citation {
    File(FileCitation),
    Web(String),
}

pub fn classify(target: &str) -> Result<Citation, String> {
    if target.chars().any(char::is_control) {
        return Err("This citation has an invalid link target".into());
    }
    let target = target.trim();
    let target = target
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(target);
    if target.is_empty() || target.len() > 16 * 1024 || target.chars().any(char::is_control) {
        return Err("This citation has an invalid link target".into());
    }
    if target
        .get(..8)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"))
        || target
            .get(..7)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("http://"))
    {
        let authority = target
            .split_once("://")
            .unwrap()
            .1
            .split(['/', '?', '#'])
            .next()
            .unwrap_or_default();
        if authority.is_empty() || authority.chars().any(char::is_whitespace) {
            return Err("This citation has an invalid web address".into());
        }
        return Ok(Citation::Web(target.into()));
    }
    let path = if target
        .get(..7)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("file://"))
    {
        let path = &target[7..];
        if let Some(local) = path.strip_prefix("localhost/") {
            format!("/{local}")
        } else if path.starts_with('/') {
            path.into()
        } else {
            return Err("Remote file links are not supported".into());
        }
    } else {
        if target.split_once(':').is_some_and(|(scheme, _)| {
            matches!(
                scheme.to_ascii_lowercase().as_str(),
                "javascript"
                    | "data"
                    | "mailto"
                    | "tel"
                    | "ssh"
                    | "ftp"
                    | "ftps"
                    | "sftp"
                    | "ws"
                    | "wss"
                    | "shell"
                    | "command"
                    | "codex"
                    | "vscode"
            )
        }) {
            return Err("This link type is not supported".into());
        }
        // A colon before any slash is a URL scheme, except for numeric :line suffixes.
        if target.starts_with("//")
            || target.split('/').next().is_some_and(|head| {
                head.split_once(':').is_some_and(|(_, rest)| {
                    !rest
                        .split(':')
                        .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                })
            })
        {
            return Err("This link type is not supported".into());
        }
        target.into()
    };
    let (path, fragment) = path
        .split_once('#')
        .map_or((path.as_str(), None), |(p, f)| (p, Some(f)));
    let mut line = None;
    let mut end_line = None;
    if let Some(fragment) = fragment {
        let fragment = fragment.strip_prefix('L').unwrap_or(fragment);
        if let Some((start, end)) = fragment.split_once('-') {
            line = positive(start);
            end_line = positive(end.strip_prefix('L').unwrap_or(end));
            if line.is_none() || end_line.is_none() || end_line < line {
                return Err("This citation has an invalid line range".into());
            }
        } else {
            line = positive(fragment);
            if line.is_none() {
                return Err("This citation has an invalid line number".into());
            }
        }
    }
    let mut path = path;
    if fragment.is_none()
        && let Some((_, last)) = path.rsplit_once(':')
        && !last.is_empty()
        && last.bytes().all(|byte| byte.is_ascii_digit())
        && positive(last).is_none()
    {
        return Err("This citation has an invalid line number".into());
    }
    if fragment.is_none()
        && let Some((before, last)) = path.rsplit_once(':')
        && let Some(number) = positive(last)
    {
        // Codex :line, as well as compiler-style :line:column targets.
        if let Some((before_line, preceding)) = before.rsplit_once(':')
            && let Some(preceding) = positive(preceding)
        {
            path = before_line;
            line = Some(preceding);
        } else {
            path = before;
            line = Some(number);
        }
    }
    let path = decode_path(path)?;
    if path.is_empty() || path.contains('?') || path.chars().any(char::is_control) {
        return Err("This citation has an invalid file path".into());
    }
    Ok(Citation::File(FileCitation {
        path: PathBuf::from(path),
        line,
        end_line,
    }))
}

fn positive(text: &str) -> Option<u64> {
    (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse::<u64>().ok())
        .flatten()
        .filter(|n| *n > 0)
}

fn decode_path(path: &str) -> Result<String, String> {
    let mut bytes = Vec::with_capacity(path.len());
    let mut iter = path.bytes();
    while let Some(byte) = iter.next() {
        if byte == b'%' {
            let high = iter.next().and_then(|b| (b as char).to_digit(16));
            let low = iter.next().and_then(|b| (b as char).to_digit(16));
            let (Some(high), Some(low)) = (high, low) else {
                return Err("This citation has an invalid escaped path".into());
            };
            bytes.push((high * 16 + low) as u8);
        } else {
            bytes.push(byte);
        }
    }
    String::from_utf8(bytes).map_err(|_| "This citation has an invalid UTF-8 path".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_targets_preserve_paths_and_locations() {
        for (url, path, line, end) in [
            ("/tmp/My File.rs:12", "/tmp/My File.rs", Some(12), None),
            ("src/main.rs#L2-L8", "src/main.rs", Some(2), Some(8)),
            ("./src/main.rs:22:3", "./src/main.rs", Some(22), None),
            (
                "file:///tmp/My%20File%23one.rs#L4",
                "/tmp/My File#one.rs",
                Some(4),
                None,
            ),
            (
                "file://localhost/tmp/caf%C3%A9.rs:3",
                "/tmp/café.rs",
                Some(3),
                None,
            ),
            ("<src/main.rs>", "src/main.rs", None, None),
        ] {
            assert_eq!(
                classify(url).unwrap(),
                Citation::File(FileCitation {
                    path: path.into(),
                    line,
                    end_line: end
                })
            );
        }
    }
    #[test]
    fn unsafe_and_malformed_targets_never_become_file_or_web_actions() {
        for target in [
            "javascript:alert(1)",
            "javascript:1",
            "TEL:123",
            "mailto:x@y.com",
            "ssh://host",
            "file://server/private",
            "//host/a",
            "https:///no-host",
            "http://bad host/x",
            "file:///tmp/%00x",
            "src/a#L0",
            "src/a:0",
            "src/a:18446744073709551616",
            "a#L9-L2",
            "a%GG",
            "a\n",
        ] {
            assert!(classify(target).is_err(), "{target}");
        }
    }
    #[test]
    fn web_links_keep_query_and_fragment() {
        let target = "https://example.com/a?q=file%3A%2F%2F#L8";
        assert_eq!(classify(target).unwrap(), Citation::Web(target.into()));
        assert_eq!(
            classify("HTTPS://example.com").unwrap(),
            Citation::Web("HTTPS://example.com".into())
        );
    }
}
