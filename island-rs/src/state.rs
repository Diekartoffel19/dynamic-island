//! The shelf survives restarts: %APPDATA%\DynamicIsland\shelf.txt, one path per line.
//! Same file and format as the C# app (UTF-8 with a byte order mark).

use std::path::PathBuf;

fn shelf_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("DynamicIsland").join("shelf.txt"))
}

pub fn load_shelf() -> Vec<String> {
    let Some(path) = shelf_path() else { return Vec::new() };
    let Ok(bytes) = std::fs::read(path) else { return Vec::new() };
    let text = String::from_utf8_lossy(&bytes);
    text.trim_start_matches('\u{feff}').lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect()
}

pub fn save_shelf(paths: &[String]) {
    let Some(path) = shelf_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut text = String::from("\u{feff}");
    for p in paths {
        text.push_str(p);
        text.push_str("\r\n");
    }
    let _ = std::fs::write(path, text);
}

fn settings_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("DynamicIsland").join("settings.txt"))
}

/// One `key=value` per line in %APPDATA%\DynamicIsland\settings.txt.
pub fn load_setting(key: &str) -> Option<String> {
    let text = std::fs::read_to_string(settings_path()?).ok()?;
    text.lines().filter_map(|l| l.split_once('=')).find(|(k, _)| k.trim() == key).map(|(_, v)| v.trim().to_string())
}

pub fn save_setting(key: &str, value: &str) {
    let Some(path) = settings_path() else { return };
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut lines: Vec<String> = text.lines().filter(|l| l.split_once('=').map(|(k, _)| k.trim() != key).unwrap_or(false)).map(str::to_string).collect();
    lines.push(format!("{key}={value}"));
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, lines.join("\r\n") + "\r\n");
}

fn reminders_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("APPDATA")?).join("DynamicIsland").join("reminders.txt"))
}

/// Reminders as (due time in Unix seconds, text), one `seconds<TAB>text` per line.
pub fn load_reminders() -> Vec<(u64, String)> {
    let Some(path) = reminders_path() else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    text.lines()
        .filter_map(|l| l.split_once('\t'))
        .filter_map(|(due, text)| Some((due.trim().parse().ok()?, text.trim().to_string())))
        .collect()
}

pub fn save_reminders(items: &[(u64, String)]) {
    let Some(path) = reminders_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text: String = items.iter().map(|(due, text)| format!("{due}\t{}\r\n", text.replace(['\t', '\r', '\n'], " "))).collect();
    let _ = std::fs::write(path, text);
}
