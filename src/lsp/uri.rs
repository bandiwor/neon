//! `file://` URIs ↔ paths.

/// `/home/me/a b.neon` → `file:///home/me/a%20b.neon`.
pub fn path_to_uri(path: &str) -> String {
    let mut out = String::from("file://");
    let path = path.replace('\\', "/");
    if !path.starts_with('/') {
        out.push('/'); // `C:/x` → `file:///C:/x`
    }
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The inverse of [`path_to_uri`]. Other schemes (`untitled:`) are returned as is.
pub fn uri_to_path(uri: &str) -> String {
    let Some(rest) = uri.strip_prefix("file://") else { return uri.to_string() };
    // `file:///C:/x` carries a drive letter after the leading slash
    let rest = match rest.as_bytes() {
        [b'/', d, b':', ..] if d.is_ascii_alphabetic() => &rest[1..],
        _ => rest,
    };
    percent_decode(rest)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = ((b[i + 1] as char).to_digit(16), (b[i + 2] as char).to_digit(16)) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for p in ["/home/me/a.neon", "/tmp/with space/é 名前.neon", "/a/b%c/#d?e.neon"] {
            let u = path_to_uri(p);
            assert!(u.starts_with("file:///"), "{u}");
            assert_eq!(uri_to_path(&u), p, "{u}");
        }
        assert_eq!(path_to_uri("/x/a b"), "file:///x/a%20b");
    }

    #[test]
    fn windows_style_and_foreign_schemes() {
        assert_eq!(uri_to_path("file:///C:/Users/me/x.neon"), "C:/Users/me/x.neon");
        assert_eq!(path_to_uri("C:\\Users\\me\\x.neon"), "file:///C:/Users/me/x.neon");
        assert_eq!(uri_to_path("untitled:Untitled-1"), "untitled:Untitled-1");
        // malformed escapes are kept literally
        assert_eq!(uri_to_path("file:///a%zzb%"), "/a%zzb%");
    }
}
