//! Current weather from OpenWeather (api.openweathermap.org, HTTPS).
//!
//! The API key is never stored in the binary. It comes from the environment
//! variable OPEN_WEATHER_API_KEY or from a `.env` file: next to the exe, in one
//! of its parent folders, or in %APPDATA%\DynamicIsland\.env.
//! Each fetch runs on a short-lived worker thread and posts `Msg::Weather`.

use std::path::PathBuf;

use windows::core::HSTRING;
use windows::Data::Json::JsonObject;
use windows::Foundation::Uri;
use windows::Web::Http::HttpClient;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

use crate::event::{post, Msg};

pub const KEY_VAR: &str = "OPEN_WEATHER_API_KEY";
/// Refresh interval while the app runs.
pub const REFRESH_MS: u32 = 15 * 60 * 1000;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Weather {
    /// Rounded °C.
    pub temp: i32,
    pub feels: i32,
    pub min: i32,
    pub max: i32,
    pub humidity: i32,
    /// km/h.
    pub wind: i32,
    /// "Leicht bewölkt".
    pub description: String,
    /// City name as OpenWeather returns it.
    pub city: String,
    /// OpenWeather icon code, e.g. "04d".
    pub icon: String,
    /// "12:30" local time of the fetch.
    pub time: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum WeatherError {
    NoKey,
    NoCity,
    BadKey,
    CityNotFound,
    Offline,
    Other(u16),
}

impl WeatherError {
    pub fn text(&self) -> String {
        match self {
            WeatherError::NoKey => format!("API-Key fehlt ({KEY_VAR} in .env)"),
            WeatherError::NoCity => "Standort nicht verfügbar · Stadt eintragen".to_string(),
            WeatherError::BadKey => "API-Key ungültig oder noch nicht aktiv".to_string(),
            WeatherError::CityNotFound => "Stadt nicht gefunden".to_string(),
            WeatherError::Offline => "Keine Verbindung".to_string(),
            WeatherError::Other(code) => format!("Fehler beim Abruf (HTTP {code})"),
        }
    }
}

/// Reads the key from the environment or the first `.env` that has it.
pub fn api_key() -> Option<String> {
    if let Ok(v) = std::env::var(KEY_VAR) {
        if !v.trim().is_empty() {
            return Some(v.trim().to_string());
        }
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        let mut dir = exe.parent().map(|p| p.to_path_buf());
        // exe sits in island-rs\target\release: walk up a few folders.
        for _ in 0..5 {
            let Some(d) = dir else { break };
            candidates.push(d.join(".env"));
            dir = d.parent().map(|p| p.to_path_buf());
        }
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        candidates.push(PathBuf::from(appdata).join("DynamicIsland").join(".env"));
    }
    candidates.iter().find_map(|path| {
        let text = std::fs::read_to_string(path).ok()?;
        text.lines().find_map(|line| {
            let line = line.trim().strip_prefix("export ").unwrap_or(line.trim());
            let (k, v) = line.split_once('=')?;
            if k.trim() != KEY_VAR {
                return None;
            }
            let v = v.trim().trim_matches(['"', '\'']).trim();
            (!v.is_empty()).then(|| v.to_string())
        })
    })
}

/// Fetches in the background; the result arrives as `Msg::Weather`.
/// Without a city the Windows location is used (if the user allows it).
pub fn fetch(city: String) {
    let city = city.trim().to_string();
    let Some(key) = api_key() else {
        post(Msg::Weather(Err(WeatherError::NoKey)));
        return;
    };
    let _ = std::thread::Builder::new().name("IslandWeather".into()).stack_size(256 * 1024).spawn(move || unsafe {
        let ok = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let query = if city.is_empty() {
            locate().map(|(lat, lon)| format!("lat={lat:.4}&lon={lon:.4}"))
        } else {
            Some(format!("q={}", encode(&city)))
        };
        post(Msg::Weather(match query {
            Some(q) => request(&q, &key),
            None => Err(WeatherError::NoCity),
        }));
        if ok {
            CoUninitialize();
        }
    });
}

fn encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b',' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Position from the Windows location service; None when it is off or denied.
fn locate() -> Option<(f64, f64)> {
    use windows::Devices::Geolocation::{GeolocationAccessStatus, Geolocator, PositionAccuracy};
    use windows::Foundation::TimeSpan;
    let access = Geolocator::RequestAccessAsync().ok()?.join().ok()?;
    if access != GeolocationAccessStatus::Allowed {
        return None;
    }
    let locator = Geolocator::new().ok()?;
    let _ = locator.SetDesiredAccuracy(PositionAccuracy::Default);
    // A position up to 30 min old is fine for the weather; wait at most 10 s.
    let max_age = TimeSpan { Duration: 30 * 60 * 10_000_000 };
    let timeout = TimeSpan { Duration: 10 * 10_000_000 };
    let pos = locator.GetGeopositionAsyncWithAgeAndTimeout(max_age, timeout).ok()?.join().ok()?;
    let p = pos.Coordinate().ok()?.Point().ok()?.Position().ok()?;
    Some((p.Latitude, p.Longitude))
}

/// `query` is "q=City" or "lat=..&lon=..".
fn request(query: &str, key: &str) -> Result<Weather, WeatherError> {
    let url = format!("https://api.openweathermap.org/data/2.5/weather?{query}&appid={}&units=metric&lang=de", encode(key));
    let uri = Uri::CreateUri(&HSTRING::from(url)).map_err(|_| WeatherError::Other(0))?;
    let client = HttpClient::new().map_err(|_| WeatherError::Offline)?;
    let response = client.GetAsync(&uri).and_then(|op| op.join()).map_err(|_| WeatherError::Offline)?;
    let status = response.StatusCode().map(|s| s.0).unwrap_or(0);
    match status {
        200 => {}
        401 => return Err(WeatherError::BadKey),
        404 => return Err(WeatherError::CityNotFound),
        code => return Err(WeatherError::Other(code as u16)),
    }
    let body = response
        .Content()
        .and_then(|c| c.ReadAsStringAsync())
        .and_then(|op| op.join())
        .map_err(|_| WeatherError::Offline)?;
    parse(&body).ok_or(WeatherError::Other(200))
}

fn parse(body: &HSTRING) -> Option<Weather> {
    let root = JsonObject::Parse(body).ok()?;
    let main = root.GetNamedObject(&HSTRING::from("main")).ok()?;
    let num = |o: &JsonObject, k: &str| o.GetNamedNumberOrDefault(&HSTRING::from(k), 0.0).unwrap_or(0.0);
    let first = root.GetNamedArray(&HSTRING::from("weather")).ok().and_then(|a| a.GetObjectAt(0).ok());
    let (description, icon) = match &first {
        Some(w) => (
            w.GetNamedStringOrDefault(&HSTRING::from("description"), &HSTRING::new()).map(|s| s.to_string_lossy()).unwrap_or_default(),
            w.GetNamedStringOrDefault(&HSTRING::from("icon"), &HSTRING::new()).map(|s| s.to_string_lossy()).unwrap_or_default(),
        ),
        None => (String::new(), String::new()),
    };
    let wind = root.GetNamedObject(&HSTRING::from("wind")).ok().map(|w| num(&w, "speed")).unwrap_or(0.0);
    let now = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    // "leichter Regen" -> "Leichter Regen"
    let mut chars = description.chars();
    let description = chars.next().map(|c| c.to_uppercase().collect::<String>() + chars.as_str()).unwrap_or_default();
    Some(Weather {
        temp: num(&main, "temp").round() as i32,
        feels: num(&main, "feels_like").round() as i32,
        min: num(&main, "temp_min").round() as i32,
        max: num(&main, "temp_max").round() as i32,
        humidity: num(&main, "humidity").round() as i32,
        wind: (wind * 3.6).round() as i32,
        description,
        city: root.GetNamedStringOrDefault(&HSTRING::from("name"), &HSTRING::new()).map(|s| s.to_string_lossy()).unwrap_or_default(),
        icon,
        time: format!("{:02}:{:02}", now.wHour, now.wMinute),
    })
}

/// Segoe icon glyph for an OpenWeather icon code.
pub fn glyph(icon: &str) -> &'static str {
    let night = icon.ends_with('n');
    match icon.get(..2).unwrap_or("") {
        "01" => {
            if night {
                "\u{E708}"
            } else {
                "\u{E706}"
            }
        }
        _ => "\u{E753}",
    }
}
