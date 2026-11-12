//! Reminders typed in plain German: "20 min Wäsche", "Wäsche in 1h 30",
//! "um 18:30 Essen", "morgen 7 uhr Müll". Parsing is pure and tested here.

/// Longest reminder that is accepted.
const MAX_SECONDS: u64 = 7 * 24 * 3600;

#[derive(Debug, PartialEq)]
pub struct Parsed {
    /// Seconds from now.
    pub seconds: u64,
    pub text: String,
}

fn unit_seconds(word: &str) -> Option<f64> {
    Some(match word {
        "s" | "sek" | "sec" | "sekunde" | "sekunden" => 1.0,
        "m" | "min" | "mins" | "minute" | "minuten" => 60.0,
        "h" | "std" | "stunde" | "stunden" | "stdn" => 3600.0,
        "d" | "tag" | "tage" | "tagen" => 86400.0,
        _ => return None,
    })
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    /// Hour and minute from "18:30".
    Clock(u32, u32),
    Word(String),
}

/// Splits "1h30min" into 1, h, 30, min and keeps "18:30" whole.
fn tokenize(input: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            let whole: String = chars[start..i].iter().collect();
            // Clock "18:30" (or "18.30" unless a unit follows: "1.30 h" is a duration).
            if i < chars.len() && (chars[i] == ':' || chars[i] == '.') {
                let digits: String = chars[i + 1..].iter().take_while(|c| c.is_ascii_digit()).collect();
                if digits.len() == 2 {
                    let h: u32 = whole.parse().unwrap_or(99);
                    let m: u32 = digits.parse().unwrap_or(99);
                    if h < 24 && m < 60 && (chars[i] == ':' || !is_unit_after(&chars, i + 3)) {
                        out.push(Tok::Clock(h, m));
                        i += 3;
                        continue;
                    }
                }
            }
            // Decimal "1,5" or "1.5".
            if i + 1 < chars.len() && (chars[i] == ',' || chars[i] == '.') && chars[i + 1].is_ascii_digit() {
                let frac_start = i + 1;
                let mut j = frac_start;
                while j < chars.len() && chars[j].is_ascii_digit() {
                    j += 1;
                }
                let frac: String = chars[frac_start..j].iter().collect();
                out.push(Tok::Num(format!("{whole}.{frac}").parse().unwrap_or(0.0)));
                i = j;
                continue;
            }
            out.push(Tok::Num(whole.parse().unwrap_or(0.0)));
        } else if c.is_alphanumeric() || c == '-' || c == '\'' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphabetic() || chars[i] == '-' || chars[i] == '\'') {
                i += 1;
            }
            if i == start {
                i += 1;
            }
            out.push(Tok::Word(chars[start..i].iter().collect()));
        } else if c.is_whitespace() {
            i += 1;
        } else {
            // Punctuation is kept as part of the text.
            out.push(Tok::Word(c.to_string()));
            i += 1;
        }
    }
    out
}

/// True if a unit word starts at `at` (after optional spaces), e.g. "1.30 h".
fn is_unit_after(chars: &[char], at: usize) -> bool {
    let rest: String = chars[at.min(chars.len())..].iter().collect();
    let word: String = rest.trim_start().chars().take_while(|c| c.is_alphabetic()).collect::<String>().to_lowercase();
    unit_seconds(&word).is_some()
}

/// `now` is the local time of day in seconds since midnight.
pub fn parse(input: &str, now: u32) -> Result<Parsed, &'static str> {
    let toks = tokenize(input.trim());
    let lower = |t: &Tok| match t {
        Tok::Word(w) => w.to_lowercase(),
        _ => String::new(),
    };

    let mut duration = 0.0f64;
    let mut clock: Option<(u32, u32)> = None;
    let mut tomorrow = false;
    let mut used = vec![false; toks.len()];
    let mut bare: Option<(usize, f64)> = None;

    let mut i = 0;
    while i < toks.len() {
        match &toks[i] {
            Tok::Clock(h, m) => {
                clock = Some((*h, *m));
                used[i] = true;
                // "18:30 uhr"
                if i + 1 < toks.len() && lower(&toks[i + 1]) == "uhr" {
                    used[i + 1] = true;
                    i += 1;
                }
            }
            Tok::Num(n) => {
                let next = toks.get(i + 1).map(lower).unwrap_or_default();
                if let Some(unit) = unit_seconds(&next) {
                    duration += n * unit;
                    used[i] = true;
                    used[i + 1] = true;
                    i += 1;
                    // "1h30": a bare number right after hours means minutes.
                    if unit == 3600.0 {
                        if let Some(Tok::Num(m)) = toks.get(i + 1) {
                            if toks.get(i + 2).map(lower).map_or(true, |w| unit_seconds(&w).is_none()) {
                                duration += m * 60.0;
                                used[i + 1] = true;
                                i += 1;
                            }
                        }
                    }
                } else if next == "uhr" && *n < 24.0 && n.fract() == 0.0 {
                    clock = Some((*n as u32, 0));
                    used[i] = true;
                    used[i + 1] = true;
                    i += 1;
                } else if bare.is_none() {
                    bare = Some((i, *n));
                }
            }
            Tok::Word(_) => {
                let w = lower(&toks[i]);
                if w == "morgen" {
                    tomorrow = true;
                    used[i] = true;
                } else if w == "übermorgen" {
                    duration += 86400.0;
                    used[i] = true;
                } else if w == "halbe" && toks.get(i + 1).map(lower).as_deref() == Some("stunde") {
                    duration += 1800.0;
                    used[i] = true;
                    used[i + 1] = true;
                    i += 1;
                }
            }
        }
        i += 1;
    }

    // A lone number without unit: minutes ("20 Wäsche").
    if duration == 0.0 && clock.is_none() {
        if let Some((index, n)) = bare {
            duration = n * 60.0;
            used[index] = true;
        }
    }

    let mut seconds = duration.round() as i64;
    if let Some((h, m)) = clock {
        let target = (h * 3600 + m * 60) as i64;
        let mut delta = target - now as i64;
        if tomorrow {
            delta += 86400;
        } else if delta <= 0 {
            delta += 86400;
        }
        seconds += delta;
    } else if tomorrow {
        if seconds == 0 {
            return Err("Uhrzeit fehlt, z. B. „morgen 7 Uhr Müll“");
        }
        seconds += 86400;
    }
    if seconds <= 0 {
        return Err("Zeit fehlt, z. B. „20 min Wäsche“ oder „um 18:30 Essen“");
    }
    if seconds as u64 > MAX_SECONDS {
        return Err("Höchstens 7 Tage im Voraus");
    }

    // Filler words around the time are dropped: "in", "um", "nach", "ab", "für".
    let filler = |i: usize| -> bool {
        let w = lower(&toks[i]);
        let next_used = used.get(i + 1).copied().unwrap_or(false);
        matches!(w.as_str(), "in" | "um" | "nach" | "ab" | "für" | "fuer" | "bis" | "eine" | "einer") && next_used
    };
    let mut words: Vec<String> = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        if used[i] || filler(i) {
            continue;
        }
        match t {
            Tok::Word(w) => words.push(w.clone()),
            Tok::Num(n) => words.push(if n.fract() == 0.0 { format!("{}", *n as i64) } else { n.to_string() }),
            Tok::Clock(h, m) => words.push(format!("{h:02}:{m:02}")),
        }
    }
    let mut text = words.join(" ");
    // No space before punctuation.
    for p in [" ,", " .", " !", " ?", " :"] {
        text = text.replace(p, &p[1..]);
    }
    let text = text.trim_matches(|c: char| c == '-' || c == ':' || c == ',' || c.is_whitespace()).to_string();
    let text = if text.is_empty() { "Erinnerung".to_string() } else { capitalize(&text) };
    Ok(Parsed { seconds: seconds as u64, text })
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// "in 5 Min", "in 1 Std 20 Min", "in 2 Tagen".
pub fn format_left(seconds: i64) -> String {
    if seconds < 60 {
        return "gleich".to_string();
    }
    let minutes = (seconds + 59) / 60;
    if minutes < 60 {
        return format!("in {minutes} Min");
    }
    let (h, m) = (minutes / 60, minutes % 60);
    if h < 24 {
        return if m == 0 { format!("in {h} Std") } else { format!("in {h} Std {m} Min") };
    }
    let days = (h + 12) / 24;
    if days == 1 {
        "morgen".to_string()
    } else {
        format!("in {days} Tagen")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> (u64, String) {
        let r = parse(s, 12 * 3600).unwrap();
        (r.seconds, r.text)
    }

    #[test]
    fn durations() {
        assert_eq!(p("20 min wäsche"), (1200, "Wäsche".into()));
        assert_eq!(p("wäsche in 20 min"), (1200, "Wäsche".into()));
        assert_eq!(p("20min Wäsche aufhängen"), (1200, "Wäsche aufhängen".into()));
        assert_eq!(p("in 1h 30 min Mama anrufen"), (5400, "Mama anrufen".into()));
        assert_eq!(p("1h30 Pizza"), (5400, "Pizza".into()));
        assert_eq!(p("1,5 std lernen"), (5400, "Lernen".into()));
        assert_eq!(p("90 sek Tee"), (90, "Tee".into()));
        assert_eq!(p("20 Wäsche"), (1200, "Wäsche".into()));
        assert_eq!(p("eine halbe stunde pause"), (1800, "Pause".into()));
        assert_eq!(p("10 minuten"), (600, "Erinnerung".into()));
    }

    #[test]
    fn clocks() {
        assert_eq!(p("um 18:30 Essen"), (6 * 3600 + 1800, "Essen".into()));
        assert_eq!(p("Essen 18.30"), (6 * 3600 + 1800, "Essen".into()));
        assert_eq!(p("13 uhr meeting"), (3600, "Meeting".into()));
        assert_eq!(p("um 9 uhr zahnarzt"), (21 * 3600, "Zahnarzt".into()));
        assert_eq!(p("morgen 7 uhr müll"), (19 * 3600, "Müll".into()));
        assert_eq!(p("morgen 14 uhr sport"), (26 * 3600, "Sport".into()));
    }

    #[test]
    fn errors() {
        assert!(parse("wäsche", 0).is_err());
        assert!(parse("", 0).is_err());
        assert!(parse("30 tage urlaub", 0).is_err());
    }

    #[test]
    fn left() {
        assert_eq!(format_left(1200), "in 20 Min");
        assert_eq!(format_left(5400), "in 1 Std 30 Min");
        assert_eq!(format_left(30), "gleich");
    }
}
