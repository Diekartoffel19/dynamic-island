//! The small network servers. Blocking sockets on plain threads, no async runtime.

pub mod airdrop;
pub mod notify_server;
pub mod qr;
pub mod share_server;

/// Percent-decodes a URL component. `+` is kept as is.
pub fn url_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Percent-encodes everything except unreserved characters (like Uri.EscapeDataString).
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Value of `key` in a query string. `+` counts as a space.
pub fn query_value(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        let Some((k, v)) = pair.split_once('=') else { continue };
        if k == key {
            return url_decode(&v.replace('+', " "));
        }
    }
    None
}
