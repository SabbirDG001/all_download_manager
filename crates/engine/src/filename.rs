use percent_encoding::percent_decode_str;

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Makes a name safe to use as a file name on Windows (and everywhere else).
pub fn sanitize_filename(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();

    // Windows does not allow trailing dots or spaces.
    let trimmed = out.trim_end_matches(['.', ' ']).trim_start().to_string();
    out = trimmed;

    if out.is_empty() {
        return "download".to_string();
    }

    let stem = out.split('.').next().unwrap_or("").to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        out.insert(0, '_');
    }

    // Keep well under the 255 character limit, preserving the extension.
    if out.chars().count() > 200 {
        let ext = out
            .rsplit_once('.')
            .map(|(_, e)| e.to_string())
            .filter(|e| e.len() <= 10);
        let head: String = out.chars().take(180).collect();
        out = match ext {
            Some(e) => format!("{head}.{e}"),
            None => head,
        };
    }
    out
}

/// Reads the file name from a `Content-Disposition` header, if it has one.
pub(crate) fn from_content_disposition(value: &str) -> Option<String> {
    let mut plain = None;
    for part in value.split(';').map(str::trim) {
        let Some((key, val)) = part.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let val = val.trim();
        if key == "filename*" {
            // RFC 5987: charset'language'percent-encoded-value
            let encoded = val.splitn(3, '\'').nth(2).unwrap_or(val);
            let decoded = percent_decode_str(encoded).decode_utf8_lossy().to_string();
            if !decoded.is_empty() {
                return Some(decoded);
            }
        } else if key == "filename" {
            let unquoted = val.trim_matches('"').to_string();
            if !unquoted.is_empty() {
                plain = Some(unquoted);
            }
        }
    }
    plain
}

/// Takes the last path segment of a URL as the file name.
pub(crate) fn from_url(url: &reqwest::Url) -> Option<String> {
    let last = url.path_segments()?.rev().find(|s| !s.is_empty())?;
    let decoded = percent_decode_str(last).decode_utf8_lossy().to_string();
    (!decoded.is_empty()).then_some(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_forbidden_characters() {
        assert_eq!(sanitize_filename("a<b>c:d\"e/f\\g|h?i*j.txt"), "a_b_c_d_e_f_g_h_i_j.txt");
    }

    #[test]
    fn handles_reserved_and_trailing() {
        assert_eq!(sanitize_filename("CON.txt"), "_CON.txt");
        assert_eq!(sanitize_filename("file. . "), "file");
        assert_eq!(sanitize_filename("..."), "download");
    }

    #[test]
    fn parses_content_disposition() {
        assert_eq!(
            from_content_disposition("attachment; filename=\"report.pdf\""),
            Some("report.pdf".into())
        );
        assert_eq!(
            from_content_disposition("attachment; filename=x.pdf; filename*=UTF-8''%E0%A6%AC.pdf"),
            Some("ব.pdf".into())
        );
    }

    #[test]
    fn parses_url() {
        let url = reqwest::Url::parse("https://example.com/files/my%20file.zip?x=1").unwrap();
        assert_eq!(from_url(&url), Some("my file.zip".into()));
        let root = reqwest::Url::parse("https://example.com/").unwrap();
        assert_eq!(from_url(&root), None);
    }
}
