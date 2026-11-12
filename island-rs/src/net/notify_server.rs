//! Local HTTP notification server on 127.0.0.1:19845 (no admin rights needed).
//! Example:
//! GET http://127.0.0.1:19845/notify?title=Build+OK&msg=Alle+Tests+gruen&type=success&progress=100

use std::io::{BufRead, BufReader, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::time::Duration;

use super::query_value;
use crate::event::{post, EventType, IslandEvent, Msg};

pub fn start(port: u16) {
    // Port already in use or restricted: carry on without the server.
    let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, port)) else { return };
    let _ = std::thread::Builder::new().name("IslandHttpNotificationServer".into()).stack_size(128 * 1024).spawn(move || {
        for stream in listener.incoming().flatten() {
            let _ = std::thread::Builder::new().stack_size(128 * 1024).spawn(move || handle(stream));
        }
    });
}

fn handle(mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_millis(2000)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(2000)));

    let mut request_line = String::new();
    {
        let mut reader = BufReader::with_capacity(1024, &stream);
        // Read as bytes: a broken encoding in the URL must not drop the request.
        let mut raw = Vec::new();
        if reader.read_until(b'\n', &mut raw).is_err() {
            return;
        }
        request_line.push_str(String::from_utf8_lossy(&raw).trim_end());
    }
    if request_line.is_empty() {
        return;
    }

    let mut parts = request_line.split(' ');
    let (Some(_method), Some(raw_url)) = (parts.next(), parts.next()) else { return };
    let query = raw_url.split_once('?').map(|(_, q)| q).unwrap_or("");
    let query = query.split('#').next().unwrap_or("");

    let title = query_value(query, "title").unwrap_or_else(|| "System-Status".to_string());
    let message = query_value(query, "msg")
        .or_else(|| query_value(query, "message"))
        .unwrap_or_else(|| "Neue Statusmeldung empfangen".to_string());
    let type_str = query_value(query, "type").unwrap_or_else(|| "info".to_string()).to_lowercase();
    let badge = query_value(query, "badge");
    let progress = query_value(query, "progress").and_then(|p| p.trim().parse::<i32>().ok()).map(|p| p.clamp(0, 100));

    let kind = match type_str.as_str() {
        "success" | "ok" => EventType::Success,
        "warn" | "warning" => EventType::Warning,
        "err" | "error" => EventType::Error,
        "audio" | "music" => EventType::Audio,
        "power" | "battery" => EventType::Power,
        "timer" => EventType::Timer,
        _ => EventType::Info,
    };

    let badge = badge.unwrap_or_else(|| type_str.to_uppercase());
    post(Msg::Alert(IslandEvent::new(&title, &message, kind, progress, &badge, 3000)));

    let json = format!("{{\"ok\":true,\"title\":\"{}\",\"message\":\"{}\"}}", escape_json(&title), escape_json(&message));
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        json.len(),
        json
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

fn escape_json(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
