//! Minimal HTTP file server for the local network. Starts on first use.
//!
//!   /{fileToken}/{name}      download one shelf file (supports Range / resume)
//!   /{hubToken}/             web page: download shelf files, upload files, send text
//!   /{hubToken}/list         shelf files as JSON
//!   /{hubToken}/upload?name= POST, body = file bytes, saved to the receive folder
//!   /{hubToken}/text         POST, body = UTF-8 text
//!
//! Nothing is reachable without one of the random tokens.
//! Uploads stream straight to disk.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::core::PCWSTR;
use windows::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

use super::{query_value, url_encode};
use crate::event::{post, Msg};
use crate::sys::network::local_address;

const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_TEXT_BYTES: u64 = 1024 * 1024;
const SHARE_PAGE: &str = include_str!("share_page.html");

#[derive(Default)]
struct Inner {
    token_to_path: HashMap<String, String>,
    /// Key is the lower-case path: Windows paths do not care about case.
    path_to_token: HashMap<String, String>,
    order: Vec<String>,
    running: bool,
}

impl Inner {
    fn token_for(&mut self, path: &str) -> String {
        let key = path.to_lowercase();
        if let Some(token) = self.path_to_token.get(&key) {
            return token.clone();
        }
        let token = create_token(16);
        self.path_to_token.insert(key, token.clone());
        self.token_to_path.insert(token.clone(), path.to_string());
        self.order.push(path.to_string());
        token
    }
}

pub struct ShareServer {
    port: u16,
    hub_token: String,
    receive_folder: PathBuf,
    inner: Mutex<Inner>,
}

struct Request {
    method: String,
    path: String,
    query: String,
    headers: HashMap<String, String>,
    /// Body bytes that arrived together with the header.
    leftover: Vec<u8>,
}

impl ShareServer {
    pub fn new(port: u16) -> Arc<ShareServer> {
        Arc::new(ShareServer {
            port,
            hub_token: create_token(8),
            receive_folder: default_receive_folder(),
            inner: Mutex::new(Inner::default()),
        })
    }

    pub fn receive_folder(&self) -> &Path {
        &self.receive_folder
    }

    /// Makes the file visible on the hub page (does not start the server).
    pub fn publish(&self, path: &str) {
        self.inner.lock().unwrap().token_for(path);
    }

    /// Registers the file and returns its direct download URL.
    pub fn share(self: &Arc<Self>, path: &str) -> io::Result<String> {
        self.ensure_started()?;
        let token = self.inner.lock().unwrap().token_for(path);
        Ok(format!("http://{}:{}/{}/{}", local_address(), self.port, token, url_encode(file_name(path))))
    }

    pub fn unshare(&self, path: &str) {
        let mut inner = self.inner.lock().unwrap();
        let key = path.to_lowercase();
        if let Some(token) = inner.path_to_token.remove(&key) {
            inner.token_to_path.remove(&token);
            inner.order.retain(|p| p.to_lowercase() != key);
        }
    }

    /// Starts the server if needed and returns the URL of the hub page.
    pub fn open_hub(self: &Arc<Self>) -> io::Result<String> {
        self.ensure_started()?;
        Ok(format!("http://{}:{}/{}/", local_address(), self.port, self.hub_token))
    }

    fn ensure_started(self: &Arc<Self>) -> io::Result<()> {
        let mut inner = self.inner.lock().unwrap();
        if inner.running {
            return Ok(());
        }
        let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, self.port))?;
        inner.running = true;

        let server = self.clone();
        std::thread::Builder::new().name("IslandFileShare".into()).stack_size(128 * 1024).spawn(move || {
            for stream in listener.incoming().flatten() {
                let server = server.clone();
                let _ = std::thread::Builder::new().stack_size(256 * 1024).spawn(move || server.handle_client(stream));
            }
        })?;
        Ok(())
    }

    // ---- Request handling ----

    fn handle_client(&self, mut stream: TcpStream) {
        let _ = stream.set_read_timeout(Some(Duration::from_millis(5000)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(30000)));
        match read_request(&mut stream) {
            Some(req) => {
                let _ = self.route(&mut stream, &req);
            }
            None => {
                let _ = write_status(&mut stream, "400 Bad Request");
            }
        }
        let _ = stream.flush();
    }

    fn route(&self, stream: &mut TcpStream, req: &Request) -> io::Result<()> {
        let segments: Vec<&str> = req.path.trim_matches('/').split('/').collect();
        let is_get = req.method == "GET";
        let is_head = req.method == "HEAD";

        if segments[0] == self.hub_token {
            let sub = segments.get(1).copied().unwrap_or("");

            if sub.is_empty() && (is_get || is_head) {
                if !req.path.ends_with('/') {
                    // Relative URLs on the page need the trailing slash.
                    return write_response(stream, "301 Moved Permanently", None, &[], false, Some(&format!("Location: /{}/\r\n", self.hub_token)));
                }
                return write_response(stream, "200 OK", Some("text/html; charset=utf-8"), SHARE_PAGE.as_bytes(), is_head, None);
            } else if sub == "list" && is_get {
                return write_response(stream, "200 OK", Some("application/json; charset=utf-8"), self.build_list_json().as_bytes(), false, None);
            } else if sub == "upload" && req.method == "POST" {
                return self.handle_upload(stream, req);
            } else if sub == "text" && req.method == "POST" {
                return self.handle_text(stream, req);
            }
            return write_status(stream, "404 Not Found");
        }

        if !is_get && !is_head {
            return write_status(stream, "405 Method Not Allowed");
        }

        let path = self.inner.lock().unwrap().token_to_path.get(segments[0]).cloned();
        match path {
            Some(path) if Path::new(&path).is_file() => self.serve_file(stream, req, &path, is_head),
            _ => write_status(stream, "404 Not Found"),
        }
    }

    fn serve_file(&self, stream: &mut TcpStream, req: &Request, path: &str, is_head: bool) -> io::Result<()> {
        let name = file_name(path).to_string();
        // Share mode: read, write and delete, so the file stays usable while it is served.
        let mut file = fs::OpenOptions::new().read(true).share_mode(7).open(path)?;
        let length = file.metadata()?.len() as i64;
        let (mut start, mut end) = (0i64, length - 1);
        let mut partial = false;

        if let Some(range) = req.headers.get("range") {
            match parse_range(range, length) {
                Range::Satisfiable(a, b) => {
                    partial = true;
                    start = a;
                    end = b;
                }
                Range::Unsatisfiable => {
                    return write_response(stream, "416 Range Not Satisfiable", None, &[], false, Some(&format!("Content-Range: bytes */{length}\r\n")));
                }
                Range::Ignored => {}
            }
        }

        let count = end - start + 1;
        let mut header = String::new();
        header.push_str(if partial { "HTTP/1.1 206 Partial Content\r\n" } else { "HTTP/1.1 200 OK\r\n" });
        header.push_str("Content-Type: application/octet-stream\r\n");
        header.push_str(&format!("Content-Length: {count}\r\n"));
        header.push_str("Accept-Ranges: bytes\r\n");
        if partial {
            header.push_str(&format!("Content-Range: bytes {start}-{end}/{length}\r\n"));
        }
        header.push_str(&format!(
            "Content-Disposition: attachment; filename=\"{}\"; filename*=UTF-8''{}\r\n",
            to_ascii_name(&name),
            url_encode(&name)
        ));
        header.push_str("X-Content-Type-Options: nosniff\r\n");
        header.push_str("Connection: close\r\n\r\n");
        stream.write_all(header.as_bytes())?;
        if is_head {
            return Ok(());
        }

        file.seek(SeekFrom::Start(start as u64))?;
        let mut buffer = vec![0u8; 64 * 1024];
        let mut remaining = count;
        while remaining > 0 {
            let want = (buffer.len() as i64).min(remaining) as usize;
            let read = file.read(&mut buffer[..want])?;
            if read == 0 {
                break;
            }
            stream.write_all(&buffer[..read])?;
            remaining -= read as i64;
        }

        if remaining == 0 && end == length - 1 {
            post(Msg::FileDownloaded(name));
        }
        Ok(())
    }

    fn handle_upload(&self, stream: &mut TcpStream, req: &Request) -> io::Result<()> {
        let Some(length) = content_length(req) else {
            return write_status(stream, "411 Length Required");
        };

        let name = sanitize_file_name(&query_value(&req.query, "name").unwrap_or_default());
        fs::create_dir_all(&self.receive_folder)?;

        if let Some(free) = free_space(&self.receive_folder) {
            if free < length {
                return write_status(stream, "507 Insufficient Storage");
            }
        }

        let temp = self.receive_folder.join(format!(".{}.part", create_token(6)));
        let result = (|| -> io::Result<()> {
            send_continue_if_expected(stream, req)?;
            let _ = stream.set_read_timeout(Some(Duration::from_millis(30000)));

            let complete = {
                let mut output = fs::OpenOptions::new().write(true).create_new(true).open(&temp)?;
                copy_body(stream, req, length, &mut output)?
            };
            if !complete {
                return write_status(stream, "400 Bad Request");
            }

            let final_path = {
                let _guard = self.inner.lock().unwrap();
                let final_path = unique_path(&self.receive_folder, &name);
                fs::rename(&temp, &final_path)?;
                final_path
            };

            write_response(stream, "200 OK", Some("application/json; charset=utf-8"), b"{\"ok\":true}", false, None)?;
            post(Msg::FileReceived(final_path));
            Ok(())
        })();

        if temp.exists() {
            let _ = fs::remove_file(&temp);
        }
        result
    }

    fn handle_text(&self, stream: &mut TcpStream, req: &Request) -> io::Result<()> {
        let Some(length) = content_length(req) else {
            return write_status(stream, "411 Length Required");
        };
        if length > MAX_TEXT_BYTES {
            return write_status(stream, "413 Payload Too Large");
        }

        send_continue_if_expected(stream, req)?;
        let mut body = Vec::with_capacity(length as usize);
        if !copy_body(stream, req, length, &mut body)? {
            return write_status(stream, "400 Bad Request");
        }

        let text = String::from_utf8_lossy(&body).into_owned();
        write_response(stream, "200 OK", Some("application/json; charset=utf-8"), b"{\"ok\":true}", false, None)?;
        if !text.is_empty() {
            post(Msg::TextReceived(text));
        }
        Ok(())
    }

    fn build_list_json(&self) -> String {
        let entries: Vec<(String, String)> = {
            let inner = self.inner.lock().unwrap();
            inner
                .order
                .iter()
                .filter_map(|path| inner.path_to_token.get(&path.to_lowercase()).map(|t| (t.clone(), path.clone())))
                .collect()
        };

        let mut json = String::from("[");
        let mut first = true;
        for (token, path) in entries {
            let Ok(meta) = fs::metadata(&path) else { continue };
            if !meta.is_file() {
                continue;
            }
            let name = file_name(&path);
            if !first {
                json.push(',');
            }
            first = false;
            json.push_str(&format!(
                "{{\"name\":\"{}\",\"size\":{},\"url\":\"/{}/{}\"}}",
                escape_json(name),
                meta.len(),
                token,
                url_encode(name)
            ));
        }
        json.push(']');
        json
    }
}

// ---- HTTP plumbing ----

fn read_request(stream: &mut TcpStream) -> Option<Request> {
    let mut buffer = vec![0u8; MAX_HEADER_BYTES];
    let mut count = 0usize;
    while count < buffer.len() {
        let read = stream.read(&mut buffer[count..]).ok()?;
        if read == 0 {
            return None;
        }
        let scan_from = count.saturating_sub(3);
        count += read;
        if let Some(pos) = buffer[scan_from..count].windows(4).position(|w| w == b"\r\n\r\n") {
            return parse_request(&buffer, scan_from + pos, count);
        }
    }
    None
}

fn parse_request(buffer: &[u8], header_end: usize, count: usize) -> Option<Request> {
    let head = String::from_utf8_lossy(&buffer[..header_end]);
    let mut lines = head.split("\r\n");
    let mut parts = lines.next()?.split(' ');
    let method = parts.next()?.to_string();
    let target = parts.next()?;
    if !target.starts_with('/') {
        return None;
    }
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target.to_string(), String::new()),
    };

    let mut headers = HashMap::new();
    for line in lines {
        if let Some((key, value)) = line.split_once(':') {
            if !key.is_empty() {
                headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
            }
        }
    }

    Some(Request { method, path, query, headers, leftover: buffer[header_end + 4..count].to_vec() })
}

fn content_length(req: &Request) -> Option<u64> {
    let value = req.headers.get("content-length")?;
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    value.parse().ok()
}

fn send_continue_if_expected(stream: &mut TcpStream, req: &Request) -> io::Result<()> {
    if let Some(expect) = req.headers.get("expect") {
        if expect.to_ascii_lowercase().contains("100-continue") {
            stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n")?;
        }
    }
    Ok(())
}

/// Copies exactly `length` body bytes; false if the peer hung up early.
fn copy_body(stream: &mut TcpStream, req: &Request, length: u64, output: &mut dyn Write) -> io::Result<bool> {
    let mut remaining = length;

    let from_header = (req.leftover.len() as u64).min(remaining) as usize;
    if from_header > 0 {
        output.write_all(&req.leftover[..from_header])?;
        remaining -= from_header as u64;
    }

    let mut buffer = vec![0u8; 64 * 1024];
    while remaining > 0 {
        let want = (buffer.len() as u64).min(remaining) as usize;
        let read = match stream.read(&mut buffer[..want]) {
            Ok(0) | Err(_) => return Ok(false),
            Ok(n) => n,
        };
        output.write_all(&buffer[..read])?;
        remaining -= read as u64;
    }
    Ok(true)
}

enum Range {
    Satisfiable(i64, i64),
    Unsatisfiable,
    /// Malformed or multi-part: the whole file is served.
    Ignored,
}

fn parse_number(s: &str) -> Option<i64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn parse_range(header: &str, length: i64) -> Range {
    if header.len() < 6 || !header[..6].eq_ignore_ascii_case("bytes=") {
        return Range::Ignored;
    }
    let spec = header[6..].trim();
    if spec.contains(',') {
        return Range::Ignored;
    }
    let Some((a, b)) = spec.split_once('-') else { return Range::Ignored };
    let (a, b) = (a.trim(), b.trim());

    if a.is_empty() {
        // Suffix range: the last N bytes.
        let Some(last) = parse_number(b) else { return Range::Ignored };
        if last <= 0 || length == 0 {
            return Range::Unsatisfiable;
        }
        return Range::Satisfiable((length - last).max(0), length - 1);
    }

    let Some(first) = parse_number(a) else { return Range::Ignored };
    let mut last = length - 1;
    if !b.is_empty() {
        let Some(value) = parse_number(b) else { return Range::Ignored };
        last = value.min(length - 1);
    }
    if first >= length || first > last {
        return Range::Unsatisfiable;
    }
    Range::Satisfiable(first, last)
}

fn write_status(stream: &mut TcpStream, status: &str) -> io::Result<()> {
    write_response(stream, status, None, &[], false, None)
}

fn write_response(stream: &mut TcpStream, status: &str, content_type: Option<&str>, body: &[u8], head_only: bool, extra_headers: Option<&str>) -> io::Result<()> {
    let mut header = format!("HTTP/1.1 {status}\r\n");
    if let Some(ct) = content_type {
        header.push_str(&format!("Content-Type: {ct}\r\n"));
    }
    header.push_str(&format!("Content-Length: {}\r\n", body.len()));
    if let Some(extra) = extra_headers {
        header.push_str(extra);
    }
    header.push_str("Cache-Control: no-store\r\n");
    header.push_str("X-Content-Type-Options: nosniff\r\n");
    header.push_str("Referrer-Policy: no-referrer\r\n");
    header.push_str("Connection: close\r\n\r\n");

    stream.write_all(header.as_bytes())?;
    if !body.is_empty() && !head_only {
        stream.write_all(body)?;
    }
    Ok(())
}

// ---- Helpers ----

/// %USERPROFILE%\Downloads\Island: files from phones and nearby PCs land here.
pub(crate) fn default_receive_folder() -> PathBuf {
    let profile = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_default();
    profile.join("Downloads").join("Island")
}

pub(crate) fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(dot) => (&name[..dot], &name[dot..]),
        None => (name, ""),
    }
}

/// Reduces a name sent by a remote device to a harmless file name (no path parts).
pub(crate) fn sanitize_file_name(raw: &str) -> String {
    let name = file_name(raw);
    let cleaned: String = name
        .chars()
        .map(|c| if (c as u32) < 32 || matches!(c, '"' | '<' | '>' | '|' | ':' | '*' | '?' | '\\' | '/') { '_' } else { c })
        .collect();
    let mut name = cleaned.trim().trim_end_matches(['.', ' ']).to_string();

    if name.chars().count() > 150 {
        let (_, ext) = split_extension(&name);
        let ext: String = if ext.chars().count() > 20 { String::new() } else { ext.to_string() };
        let keep = 150 - ext.chars().count();
        name = name.chars().take(keep).collect::<String>() + &ext;
    }
    if name.is_empty() {
        name = "Datei".to_string();
    }

    // Windows device names (CON, NUL, COM1 ...) are not usable as file names.
    let stem = name.split('.').next().unwrap_or("").trim().to_uppercase();
    let numbered = stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || numbered {
        name.insert(0, '_');
    }
    name
}

pub(crate) fn unique_path(folder: &Path, name: &str) -> PathBuf {
    let mut candidate = folder.join(name);
    let (stem, ext) = split_extension(name);
    let mut i = 2;
    while candidate.exists() {
        candidate = folder.join(format!("{stem} ({i}){ext}"));
        i += 1;
    }
    candidate
}

fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 32 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn to_ascii_name(name: &str) -> String {
    name.chars().map(|c| if (' '..'\u{7f}').contains(&c) && c != '"' && c != '\\' { c } else { '_' }).collect()
}

pub(crate) fn create_token(byte_count: usize) -> String {
    let mut bytes = vec![0u8; byte_count];
    unsafe {
        let _ = BCryptGenRandom(None, &mut bytes, BCRYPT_USE_SYSTEM_PREFERRED_RNG);
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub(crate) fn free_space(folder: &Path) -> Option<u64> {
    let wide: Vec<u16> = folder.to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
    let mut free = 0u64;
    unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut free), None, None).ok()? };
    Some(free)
}
