//! The island scene: fixed layout, drawn directly. Sizes, radii and colours
//! come from MainWindow.xaml and App.xaml of the C# app.
//!
//! Drawing also records the hit rectangles of every widget, so the mouse
//! code always tests against what is on screen.

use super::{accent, accent_index, argb, line_height, rgb8, ts, Color, Gfx, Paint, Rect, Ts, ACCENTS};
use crate::event::EventType;
use crate::island::{format_seconds, format_size, to_hex, to_hsl, to_rgb, App, Hit, Id, Key, NOTE_POP, PEER_POP, TAB_COUNT, TAB_W, REM_POP, SWITCH_GLOW, SWITCH_HOTKEY, VisualState, due_text, unix_now};
use crate::settings::{hotkey_text, SETTINGS_TAB_COUNT, SW_H, SW_W, TITLE_H};
use crate::sys::media::format_ms;
use crate::sys::weather;
use crate::reminder::format_left;
use crate::sys::radio::Status as RadioStatus;
use crate::shell::picker::LOUPE;

/// Window size in device independent pixels.
pub const WIN_W: f32 = 560.0;
pub const WIN_H: f32 = 400.0;
/// Horizontal centre of the island.
pub const CX: f32 = WIN_W / 2.0;
/// Room around the island that stays part of the window, for the shadow.
pub const REGION_MARGIN: f32 = 40.0;


/// Tab content area of the expanded view.
const CONTENT_X: f32 = 88.0;
const CONTENT_Y: f32 = 58.0;
pub const CONTENT_W: f32 = 384.0;
const CONTENT_H: f32 = 124.0;
const CONTENT_R: f32 = CONTENT_X + CONTENT_W;

/// Timeline of the media tab (hit area; the track is drawn in its middle).
pub const MEDIA_BAR: Rect = Rect::new(136.0, 190.0, 288.0, 18.0);
/// Volume slider in the start tab footer, including the room for the thumb.
pub const SLIDER: Rect = Rect::new(310.0, 196.0, 134.0, 20.0);
/// Footer row of the start tab.
const FOOTER_Y: f32 = 192.0;
/// Orange of the notification bell, as in the compact pill.
const BELL: Color = argb(0xFFFF9F0A);
/// Width of a file chip plus its gap.
pub const CHIP_PITCH: f32 = 88.0;
/// Nearby PCs shown in the picker.
pub const MAX_PEERS: usize = 5;
/// Bar heights of the compact pill before the first animation.
pub const EQ_REST: [f32; 4] = [0.4, 0.8, 0.55, 0.3];

const WHITE: Color = argb(0xFFFFFFFF);
const BLACK: Color = argb(0xFF000000);
const TEXT2: Color = argb(0x99FFFFFF);
const TEXT3: Color = argb(0x59FFFFFF);
const CARD: Color = argb(0x14FFFFFF);
const TRACK: Color = argb(0x24FFFFFF);
const CHIP_TEXT: Color = argb(0xD9FFFFFF);
const HOVER: Color = argb(0x1FFFFFFF);

fn event_color(kind: EventType) -> Color {
    match kind {
        EventType::Success => rgb8(48, 209, 88),
        EventType::Warning => rgb8(255, 159, 10),
        EventType::Error => rgb8(255, 69, 58),
        _ => accent(),
    }
}

fn event_glyph(kind: EventType) -> &'static str {
    match kind {
        EventType::Success => "\u{E73E}",
        EventType::Warning => "\u{E7BA}",
        EventType::Error => "\u{E711}",
        EventType::Audio => "\u{E767}",
        EventType::Power => "\u{E945}",
        EventType::Network => "\u{E774}",
        EventType::Clipboard => "\u{E8C8}",
        EventType::KeyLock => "\u{E765}",
        EventType::Bluetooth => "\u{E702}",
        EventType::Usb => "\u{E88E}",
        EventType::Timer => "\u{E916}",
        EventType::Nearby => "\u{E72D}",
        EventType::Info => "\u{E946}",
    }
}

struct Cx<'a> {
    g: &'a mut Gfx,
    a: &'a mut App,
    /// Widgets only take clicks while their view is the active one.
    hit: bool,
}

pub fn draw(g: &mut Gfx, a: &mut App) {
    a.hits.clear();
    let (w, h, r) = a.shape();
    let x0 = CX - w / 2.0;
    // Click-through mode shows the island slightly faded.
    let base = if a.click_through { 0.8 } else { 1.0 };

    let glow = if a.glow_mode == 1 { None } else { a.glow_color() };

    g.begin();
    g.alpha = base;
    g.island_base(x0, w, h, r);

    // Content is clipped to the island body; inner glow stays strictly inside the rim.
    let body = Rect::new(x0, -r, w, h + r);
    let mask = g.body_mask(body, r);
    if let Some(c) = glow {
        let (cy, ry, strength) = if h < 100.0 { (h * 0.75, h * 1.15, 0.42) } else { (h * 0.65, h * 0.85, 0.26) };
        g.push_mask(mask.as_ref(), 1.0);
        g.fill_radial_glow(Rect::new(x0, 0.0, w, h), CX, cy, w * 0.60, ry, c.alpha((strength * c.a).min(0.85)));
        g.pop_layer();
        g.stroke_rrect(body.inset(0.5), (r - 0.5).max(0.0), argb(0x1AFFFFFF), 1.0, false);
    }
    let interactive = !a.click_through;
    let mut cx = Cx { g, a, hit: false };

    for v in 0..3usize {
        let k = v as u8;
        let current = cx.a.state as usize == v;
        let opacity = cx.a.anims.get(Key::ViewOpacity(k), if v == 0 { 1.0 } else { 0.0 });
        if opacity <= 0.004 {
            continue;
        }
        let scale = cx.a.anims.get(Key::ViewScale(k), 1.0);
        let blur = cx.a.anims.get(Key::ViewBlur(k), 0.0);
        // Height the view is laid out for: it scales around its own centre.
        let vh = match v {
            0 => 34.0,
            1 => toast_height(cx.a),
            _ => cx.a.expanded_height(),
        };
        cx.hit = current && interactive;

        if blur > 0.25 {
            cx.g.begin_offscreen();
            cx.g.alpha = 1.0;
            cx.view(v);
            cx.g.end_offscreen();

            cx.g.push_mask(mask.as_ref(), opacity * base);
            cx.g.push_scale(CX, vh / 2.0, scale, scale);
            cx.g.draw_offscreen_blurred(blur / 3.0);
            cx.g.pop_xf();
            cx.g.pop_layer();
        } else {
            cx.g.push_mask(mask.as_ref(), 1.0);
            cx.g.push_scale(CX, vh / 2.0, scale, scale);
            cx.g.alpha = opacity * base;
            cx.view(v);
            cx.g.pop_xf();
            cx.g.pop_layer();
        }
    }
    cx.g.alpha = 1.0;
}

fn toast_height(a: &App) -> f32 {
    if a.toast_hud {
        40.0
    } else {
        58.0
    }
}

impl Cx<'_> {
    fn view(&mut self, v: usize) {
        match v {
            0 => self.compact(),
            1 => self.toast(),
            _ => self.expanded(),
        }
    }

    fn add_hit(&mut self, id: Id, rect: Rect) {
        if self.hit {
            self.a.hits.push(Hit { id, rect });
        }
    }

    fn anim(&self, key: Key, default: f32) -> f32 {
        self.a.anims.get(key, default)
    }

    // ---- Widgets ----

    /// Round icon button: hover fade + press scale.
    fn icon_button(&mut self, id: Id, r: Rect, glyph: &str, size: f32, fg: Color, bg: Option<Paint>) {
        let hover = self.anim(Key::Hover(id), 0.0);
        let press = self.anim(Key::Press(id), 1.0);
        self.g.push_scale(r.cx(), r.cy(), press, press);
        if let Some(bg) = bg {
            self.g.fill_rrect(r, r.h / 2.0, bg);
        }
        if hover > 0.0 {
            self.g.fill_rrect(r, r.h / 2.0, Paint::Solid(HOVER.fade(hover)));
        }
        self.g.text(glyph, r, ts(size).icon().center().vcenter(), fg);
        self.g.pop_xf();
        self.add_hit(id, r);
    }

    /// Pill or chip with text and an optional leading icon. Returns its rectangle.
    /// `x` is the left edge, or the right edge when `from_right` is set.
    #[allow(clippy::too_many_arguments)]
    fn label_button(&mut self, id: Id, x: f32, y: f32, h: f32, pad: f32, min_w: f32, label: &str, st: Ts, fg: Color, bg: Paint, icon: Option<&str>, from_right: bool) -> Rect {
        let text_w = self.g.measure(label, st);
        let icon_w = if icon.is_some() { 12.0 + 6.0 } else { 0.0 };
        let w = (pad * 2.0 + icon_w + text_w).ceil().max(min_w);
        let r = Rect::new(if from_right { x - w } else { x }, y, w, h);

        let hover = self.anim(Key::Hover(id), 0.0);
        let press = self.anim(Key::Press(id), 1.0);
        self.g.push_scale(r.cx(), r.cy(), press, press);
        self.g.fill_rrect(r, h / 2.0, bg);
        if hover > 0.0 {
            self.g.fill_rrect(r, h / 2.0, Paint::Solid(HOVER.fade(hover)));
        }
        let content_w = icon_w + text_w;
        let mut tx = r.x + (w - content_w) / 2.0;
        if let Some(icon) = icon {
            self.g.text(icon, Rect::new(tx, r.y + 1.0, 12.0, h), ts(12.0).icon().vcenter(), fg);
            tx += icon_w;
        }
        self.g.text(label, Rect::new(tx, r.y, text_w + 2.0, h), st.vcenter(), fg);
        self.g.pop_xf();
        self.add_hit(id, r);
        r
    }

    fn chip(&mut self, id: Id, x: f32, y: f32, h: f32, label: &str) -> Rect {
        self.label_button(id, x, y, h, 10.0, 0.0, label, ts(11.0), CHIP_TEXT, Paint::Solid(CARD), None, false)
    }

    /// Thin progress bar: track plus a fill that grows from the left.
    fn bar(&mut self, r: Rect, fraction: f32, fill: Paint) {
        self.g.fill_rrect(r, r.h / 2.0, Paint::Solid(TRACK));
        self.g.fill_rrect(Rect::new(r.x, r.y, r.w * fraction.clamp(0.0, 1.0), r.h), r.h / 2.0, fill);
    }

    // =====================================================================
    //  1. Compact
    // =====================================================================

    fn compact(&mut self) {
        // Laid out on the pill's current width, so the items ride with the spring.
        let w = if self.a.state == VisualState::Compact { self.a.shape().0 } else { self.a.compact_width() as f32 };
        let x0 = CX - w / 2.0;
        // The clock sits in the middle of the pill, right under the screen centre.
        let clock = self.a.clock_text.clone();
        self.g.text(&clock, Rect::new(CX - 40.0, 0.0, 80.0, 33.0), ts(12.5).bold().center().vcenter(), WHITE);

        // Weather on the left side: glyph and temperature.
        if let Some((glyph, temp)) = self.a.compact_weather() {
            let st = ts(12.0).bold();
            let tw = self.g.measure(&temp, st).ceil();
            let left = x0 + 16.0;
            self.g.text(glyph, Rect::new(left, 1.0, 14.0, 33.0), ts(11.5).icon().vcenter(), TEXT2);
            self.g.text(&temp, Rect::new(left + 18.0, 0.0, tw + 2.0, 33.0), st.vcenter(), WHITE);
        }

        // Status items from the right edge; widths match App::compact_width.
        const GAP: f32 = 10.0;
        let mut right = x0 + w - 16.0;

        // Privacy LEDs: green = camera, orange = microphone.
        let leds = [(self.a.cam_on, rgb8(48, 209, 88)), (self.a.mic_on, BELL)];
        if leds.iter().any(|l| l.0) {
            for (on, color) in leds {
                if on {
                    self.g.fill_ellipse(right - 3.0, 17.0, 6.0, Paint::Solid(color.fade(0.22)));
                    self.g.fill_ellipse(right - 3.0, 17.0, 3.0, Paint::Solid(color));
                    right -= 10.0;
                }
            }
            right += 4.0 - GAP;
        }

        if self.a.focus_running {
            let text = format_seconds(self.a.focus_remaining);
            let st = ts(12.5).bold();
            let tw = self.g.measure(&text, st).ceil();
            self.g.text(&text, Rect::new(right - tw, 0.0, tw + 2.0, 33.0), st.vcenter(), accent());
            right -= 40.0f32.max(tw) + GAP;
        } else {
            // Music bars: only while something plays, fading in and out.
            let vis = self.anim(Key::MusicVis, if self.a.music { 1.0 } else { 0.0 });
            if vis > 0.004 {
                let saved = self.g.alpha;
                self.g.alpha = saved * vis;
                let bx = right - 19.5;
                for i in 0..4usize {
                    let s = self.anim(Key::Eq(i as u8), EQ_REST[i]) * (0.4 + 0.6 * vis);
                    let bar_h = 14.0 * s;
                    self.g.fill_rrect(Rect::new(bx + i as f32 * 5.5, 17.0 - bar_h / 2.0, 3.0, bar_h), 1.5, Paint::Grad);
                }
                self.g.alpha = saved;
            }
            if self.a.music {
                right -= 19.5 + GAP;
            }
        }

        // Next reminder: bell and its time.
        if let Some(due) = self.a.reminders.first().map(|r| r.due) {
            let text = due_text(due);
            let st = ts(12.0).bold();
            let tw = self.g.measure(&text, st).ceil();
            self.g.text(&text, Rect::new(right - tw, 0.0, tw + 2.0, 33.0), st.vcenter(), BELL);
            self.g.text("\u{EA8F}", Rect::new(right - tw - 16.0, 1.0, 13.0, 33.0), ts(11.0).icon().vcenter(), BELL);
            right -= tw + 16.0 + GAP;
        }

        // Unread notifications: only a number when there really are some.
        let unread = self.a.unread_shown();
        if unread > 0 && self.a.opt_bell {
            let text = unread.to_string();
            let st = ts(11.5).bold();
            let tw = self.g.measure(&text, st).ceil();
            self.g.text(&text, Rect::new(right - tw, 0.0, tw + 2.0, 33.0), st.vcenter(), accent());
            self.g.text("\u{E8BD}", Rect::new(right - tw - 17.0, 1.0, 14.0, 33.0), ts(11.0).icon().vcenter(), accent());
        }
    }

    // =====================================================================
    //  2. Toast
    // =====================================================================

    fn toast(&mut self) {
        // New content slides in from slightly below while the toast stays open.
        let swap = self.anim(Key::ToastSwap, 1.0);
        let saved = self.g.alpha;
        self.g.alpha = saved * swap;
        self.g.push_translate(0.0, (1.0 - swap) * 6.0);
        if self.a.toast_hud {
            self.toast_hud();
        } else {
            self.toast_card();
        }
        self.g.pop_xf();
        self.g.alpha = saved;
    }

    /// Volume: speaker, level bar, percent on one line.
    fn toast_hud(&mut self) {
        let x = CX - 150.0;
        let muted = self.a.toast.badge == "MUTE";
        let glyph = if muted { "\u{E74F}" } else { "\u{E767}" };
        self.g.text(glyph, Rect::new(x + 16.0, 0.0, 20.0, 40.0), ts(14.0).icon().center().vcenter(), WHITE);
        let fraction = self.anim(Key::Bar(3), 0.0);
        self.bar(Rect::new(x + 46.0, 18.0, 186.0, 4.0), fraction, if muted { Paint::Solid(TEXT3) } else { Paint::GradH });
        let label = if muted { "Stumm".to_string() } else { self.a.toast.badge.clone() };
        self.g.text(&label, Rect::new(x + 236.0, 0.0, 48.0, 40.0), ts(12.5).bold().right().vcenter(), if muted { TEXT2 } else { WHITE });
    }

    fn toast_card(&mut self) {
        let x = CX - 172.0;
        let accent = event_color(self.a.toast.kind);
        let glyph = event_glyph(self.a.toast.kind);
        let (title, message, badge) = (self.a.toast.title.clone(), self.a.toast.message.clone(), self.a.toast.badge.clone());

        let icon = Rect::new(x + 11.0, 11.0, 36.0, 36.0);
        self.g.fill_ellipse(icon.cx(), icon.cy(), 18.0, Paint::Solid(accent.alpha(46.0 / 255.0)));
        self.g.text(glyph, icon, ts(15.0).icon().center().vcenter(), accent);

        let content_right = x + 344.0 - 20.0;
        let badge_w = if self.a.toast.offer.is_some() {
            // Incoming nearby offer: decline and accept instead of the badge.
            let decline = Rect::new(content_right - 30.0, 14.0, 30.0, 30.0);
            let accept = Rect::new(decline.x - 36.0, 14.0, 30.0, 30.0);
            self.icon_button(Id::OfferAccept, accept, "\u{E73E}", 12.0, WHITE, Some(Paint::Solid(rgb8(48, 209, 88))));
            self.icon_button(Id::OfferDecline, decline, "\u{E711}", 11.0, WHITE, Some(Paint::Solid(argb(0x33FFFFFF))));
            content_right - accept.x
        } else {
            let badge_style = ts(12.0).bold();
            let badge_w = self.g.measure(&badge, badge_style).ceil();
            self.g.text(&badge, Rect::new(content_right - badge_w, 0.0, badge_w + 2.0, 58.0), badge_style.vcenter(), accent);
            badge_w
        };

        let tx = icon.right() + 11.0;
        let tw = content_right - badge_w - 10.0 - tx;
        let (title_h, message_h) = (line_height(13.0), line_height(11.5));
        let has_progress = self.a.toast.has_progress;
        let total = title_h + message_h + if has_progress { 8.0 } else { 0.0 };
        let y0 = (58.0 - total) / 2.0;

        self.g.text(&title, Rect::new(tx, y0, tw, title_h + 2.0), ts(13.0).bold().trim(), WHITE);
        self.g.text(&message, Rect::new(tx, y0 + title_h, tw, message_h + 2.0), ts(11.5).trim(), TEXT2);
        if has_progress {
            let fraction = self.anim(Key::Bar(3), 0.0);
            self.bar(Rect::new(tx, y0 + title_h + message_h + 5.0, tw, 3.0), fraction, Paint::Solid(accent));
        }
    }

    // =====================================================================
    //  3. Expanded
    // =====================================================================

    fn expanded(&mut self) {
        self.header();

        let view_hit = self.hit;
        let saved_alpha = self.g.alpha;
        for i in 0..TAB_COUNT {
            let k = i as u8;
            let opacity = self.anim(Key::TabOpacity(k), if i == self.a.tab { 1.0 } else { 0.0 });
            if opacity <= 0.004 {
                continue;
            }
            let shift = self.anim(Key::TabShift(k), 0.0);
            self.hit = view_hit && i == self.a.tab;
            self.g.alpha = saved_alpha * opacity;
            self.g.push_translate(shift, 0.0);
            match i {
                0 => self.tab_home(),
                1 => self.tab_media(),
                2 => self.tab_color(),
                3 => self.tab_files(),
                4 => self.tab_focus(),
                5 => self.tab_reminders(),
                _ => self.tab_controls(),
            }
            self.g.pop_xf();
        }
        self.g.alpha = saved_alpha;
        self.hit = view_hit;
    }

    /// Clock, tab bar and pin button.
    fn header(&mut self) {
        // The start tab shows its own big clock: the small one fades out there.
        let home = self.anim(Key::TabOpacity(0), if self.a.tab == 0 { 1.0 } else { 0.0 });
        let saved = self.g.alpha;
        self.g.alpha = saved * (1.0 - home);
        if home < 0.996 {
            let clock = self.a.clock_text.clone();
            let date = self.a.date_text.clone();
            let clock_style = ts(20.0).bold();
            let stack_y = 14.0 + (32.0 - line_height(20.0)) / 2.0;
            let clock_w = self.g.measure(&clock, clock_style);
            self.g.text(&clock, Rect::new(90.0, stack_y, clock_w + 4.0, 30.0), clock_style, WHITE);
            let date_y = stack_y + line_height(20.0) - 3.0 - line_height(11.5);
            // The date gives way to the tab bar.
            let date_x = 90.0 + clock_w + 8.0;
            let bar_left = 402.0 - (TAB_COUNT as f32 * TAB_W + 4.0);
            self.g.text(&date, Rect::new(date_x, date_y, (bar_left - 6.0 - date_x).max(0.0), 18.0), ts(11.5).trim(), TEXT2);
        }
        self.g.alpha = saved;

        // Segmented tab bar inside a 30 high container with 2 padding, right edge at 402.
        let bar_w = TAB_COUNT as f32 * TAB_W + 4.0;
        let bar = Rect::new(402.0 - bar_w, 15.0, bar_w, 30.0);
        self.g.fill_rrect(bar, 15.0, Paint::Solid(CARD));
        let highlight_x = self.anim(Key::TabHighlight, self.a.tab as f32 * TAB_W);
        self.g.fill_rrect(Rect::new(bar.x + 2.0 + highlight_x, 17.0, TAB_W, 26.0), 13.0, Paint::Solid(argb(0x26FFFFFF)));
        const TAB_GLYPHS: [&str; TAB_COUNT] = ["\u{E80F}", "\u{E8D6}", "\u{E790}", "\u{E8A5}", "\u{E916}", "\u{E823}", "\u{E701}"];
        for (i, glyph) in TAB_GLYPHS.iter().enumerate() {
            let id = Id::Tab(i as u8);
            let r = Rect::new(bar.x + 2.0 + i as f32 * TAB_W, 17.0, TAB_W, 26.0);
            let color = if i == self.a.tab {
                WHITE
            } else if self.a.hot == id {
                argb(0xCCFFFFFF)
            } else {
                argb(0x80FFFFFF)
            };
            self.g.text(glyph, r, ts(13.0).icon().center().vcenter(), color);
            self.add_hit(id, r);
        }

        // Settings open in a window of their own.
        self.icon_button(Id::OpenSettings, Rect::new(408.0, 15.0, 30.0, 30.0), "\u{E713}", 12.0, TEXT2, Some(Paint::Solid(CARD)));

        let pinned = self.a.pinned;
        self.icon_button(
            Id::Pin,
            Rect::new(442.0, 15.0, 30.0, 30.0),
            if pinned { "\u{E840}" } else { "\u{E718}" },
            12.0,
            if pinned { WHITE } else { TEXT2 },
            Some(if pinned { Paint::Grad } else { Paint::Solid(CARD) }),
        );
    }

    // ---- Tab 0: overview ----

    fn tab_home(&mut self) {
        // Big clock and the long date, top left.
        let clock = self.a.clock_text.clone();
        self.g.text(&clock, Rect::new(86.0, 4.0, 200.0, 56.0), ts(40.0).bold(), WHITE);
        let date = self.a.date_long.clone();
        self.g.text(&date, Rect::new(89.0, 58.0, 180.0, 18.0), ts(12.0).trim(), TEXT2);

        // Under the tab bar, top right: the weather, or the system line without it.
        if let Some(w) = self.a.weather.clone() {
            let temp = format!("{}°", w.temp);
            let st = ts(17.0).bold();
            let tw = self.g.measure(&temp, st).ceil();
            self.g.text(&temp, Rect::new(472.0 - tw - 2.0, 48.0, tw + 4.0, 24.0), st, WHITE);
            self.g.text(weather::glyph(&w.icon), Rect::new(472.0 - tw - 26.0, 52.0, 20.0, 20.0), ts(14.0).icon(), WHITE);
            let line = format!("{} · {}", w.description, w.city);
            self.g.text(&line, Rect::new(280.0, 72.0, 192.0, 16.0), ts(10.5).right().trim(), argb(0xFF6CB8FF));
        } else {
            let load = format!("CPU {} %  ·  RAM {} %", self.a.tel.cpu, self.a.tel.ram);
            self.g.text(&load, Rect::new(280.0, 52.0, 192.0, 16.0), ts(11.0).right().trim(), TEXT2);
            let net = self.a.net_name.clone();
            self.g.text(&net, Rect::new(280.0, 67.0, 192.0, 16.0), ts(11.0).right().trim(), argb(0xFF6CB8FF));
        }

        self.notices_row(Rect::new(CONTENT_X, 88.0, CONTENT_W, 46.0));

        // Quick actions.
        let quick = [
            (Id::Quick(0), "\u{E8EF}", "Rechner"),
            (Id::Quick(1), "\u{EF3C}", "Farbe"),
            (Id::Quick(2), "\u{E77F}", "Zwischenablage"),
            (Id::Quick(3), "\u{E823}", "Erinnerung"),
        ];
        let mut x = CONTENT_X;
        for (id, glyph, label) in quick {
            let r = self.label_button(id, x, 144.0, 28.0, 11.0, 0.0, label, ts(11.5), CHIP_TEXT, Paint::Solid(CARD), Some(glyph), false);
            x = r.right() + 6.0;
        }

        self.g.fill_rect(Rect::new(CONTENT_X, 182.0, CONTENT_W, 1.0), Paint::Solid(argb(0x14FFFFFF)));

        // Footer: media keys and battery on the left, volume on the right.
        let y = FOOTER_Y;
        self.icon_button(Id::Prev, Rect::new(86.0, y, 28.0, 28.0), "\u{E892}", 11.0, WHITE, None);
        let play = if self.a.media.playing { "\u{E769}" } else { "\u{E768}" };
        self.icon_button(Id::Play, Rect::new(116.0, y, 28.0, 28.0), play, 11.0, WHITE, Some(Paint::Grad));
        self.icon_button(Id::Next, Rect::new(146.0, y, 28.0, 28.0), "\u{E893}", 11.0, WHITE, None);

        let tel = &self.a.tel;
        let (glyph, text, color) = if tel.has_battery {
            let glyph = if tel.charging {
                "\u{E945}"
            } else {
                const LEVELS: [&str; 11] =
                    ["\u{E850}", "\u{E851}", "\u{E852}", "\u{E853}", "\u{E854}", "\u{E855}", "\u{E856}", "\u{E857}", "\u{E858}", "\u{E859}", "\u{E83F}"];
                LEVELS[(tel.battery.clamp(0, 100) / 10) as usize]
            };
            let low = !tel.charging && tel.battery <= 20;
            (glyph, format!("{} %", tel.battery), if low { rgb8(255, 69, 58) } else { WHITE })
        } else {
            ("\u{E945}", "Netzbetrieb".to_string(), TEXT2)
        };
        self.g.text(glyph, Rect::new(188.0, y + 1.0, 18.0, 28.0), ts(13.0).icon().vcenter(), color);
        self.g.text(&text, Rect::new(210.0, y, 70.0, 28.0), ts(11.5).bold().vcenter().trim(), color);

        let muted = self.a.tel.muted;
        self.icon_button(Id::Mute, Rect::new(278.0, y, 28.0, 28.0), if muted { "\u{E74F}" } else { "\u{E767}" }, 12.0, if muted { TEXT2 } else { WHITE }, None);
        let volume = self.a.tel.volume.clamp(0, 100);
        let track = Rect::new(SLIDER.x, SLIDER.cy() - 2.0, SLIDER.w, 4.0);
        let thumb_x = SLIDER.x + 7.0 + (SLIDER.w - 14.0) * volume as f32 / 100.0;
        self.g.fill_rrect(track, 2.0, Paint::Solid(TRACK));
        self.g.fill_rrect(Rect::new(track.x, track.y, thumb_x - track.x, 4.0), 2.0, if muted { Paint::Solid(TEXT3) } else { Paint::GradH });
        let thumb = if self.a.hot == Id::Slider { 7.5 } else { 6.5 };
        self.g.fill_ellipse(thumb_x, SLIDER.cy(), thumb, Paint::Solid(WHITE));
        self.add_hit(Id::Slider, SLIDER);
        self.g.text(&volume.to_string(), Rect::new(446.0, y, 26.0, 28.0), ts(11.5).bold().right().vcenter(), TEXT2);
    }

    /// The two newest notifications as cards; "+N weitere" when there are more.
    fn notices_row(&mut self, area: Rect) {
        let count = self.a.notices.len();
        if count == 0 {
            self.g.fill_rrect(area, 14.0, Paint::Solid(argb(0x08FFFFFF)));
            self.g.stroke_rrect(area.inset(0.5), 13.5, argb(0x26FFFFFF), 1.0, true);
            let label = "Keine neuen Benachrichtigungen";
            let st = ts(11.5);
            let tw = self.g.measure(label, st).ceil();
            let x = area.cx() - (tw + 20.0) / 2.0;
            self.g.text("\u{EA8F}", Rect::new(x, area.y + 1.0, 14.0, area.h), ts(12.0).icon().vcenter(), TEXT3);
            self.g.text(label, Rect::new(x + 20.0, area.y, tw + 2.0, area.h), st.vcenter(), TEXT3);
            return;
        }
        let shown = count.min(2);
        let w = if shown == 1 { area.w } else { (area.w - 6.0) / 2.0 };
        for i in 0..shown {
            let n = &self.a.notices[i];
            let (id, kind, title, message, time) = (n.id, n.kind, n.title.clone(), n.message.clone(), n.time.clone());
            let extra = if i == 1 { count - 2 } else { 0 };
            let sub = if extra > 0 { format!("+{extra} weitere · {message}") } else { message };
            let r = Rect::new(area.x + i as f32 * (w + 6.0), area.y, w, area.h);
            self.notice_card(id, r, kind, &title, &sub, &time);
        }
    }

    fn notice_card(&mut self, id: u32, r: Rect, kind: EventType, title: &str, sub: &str, time: &str) {
        let hit_id = Id::Notice(id);
        let pop = self.anim(Key::PopScale(NOTE_POP + id), 1.0);
        let press = self.anim(Key::Press(hit_id), 1.0);
        let hover = self.anim(Key::Hover(hit_id), 0.0);
        let saved = self.g.alpha;
        self.g.alpha = saved * self.anim(Key::PopOpacity(NOTE_POP + id), 1.0);
        let s = pop * press;
        self.g.push_scale(r.cx(), r.cy(), s, s);

        self.g.fill_rrect(r, 14.0, Paint::Solid(CARD));
        if hover > 0.0 {
            self.g.fill_rrect(r, 14.0, Paint::Solid(HOVER.fade(hover)));
        }
        let color = match kind {
            EventType::Success | EventType::Warning | EventType::Error => event_color(kind),
            _ => BELL,
        };
        self.g.text(event_glyph(kind), Rect::new(r.x + 13.0, r.y + 1.0, 16.0, r.h), ts(12.0).icon().vcenter(), color);

        let tx = r.x + 37.0;
        let tw = r.right() - tx - 11.0;
        let time_st = ts(10.0);
        let time_w = self.g.measure(time, time_st).ceil();
        self.g.text(time, Rect::new(r.right() - 11.0 - time_w, r.y + 8.0, time_w + 2.0, 16.0), time_st, TEXT3);
        self.g.text(title, Rect::new(tx, r.y + 6.0, (tw - time_w - 6.0).max(0.0), 18.0), ts(12.0).bold().trim(), WHITE);
        self.g.text(sub, Rect::new(tx, r.y + 24.0, tw, 16.0), ts(10.5).trim(), TEXT2);

        self.g.pop_xf();
        self.g.alpha = saved;
        self.add_hit(hit_id, r);
    }

    // ---- Tab 1: now playing ----

    fn tab_media(&mut self) {
        let m = self.a.media.clone();

        // Cover bitmap lives on the GPU device; built from the pixels on first use.
        if self.a.cover_bitmap.is_none() {
            if let Some(c) = &self.a.cover {
                self.a.cover_bitmap = self.g.create_bitmap(c.size, c.size, &c.pixels);
            }
        }
        let bitmap = self.a.cover_bitmap.clone();

        // Soft glow radiating from the cover behind the media controls (only inside and when enabled).
        let cover = Rect::new(CONTENT_X, CONTENT_Y, 124.0, 124.0);
        if self.a.opt_glow {
            let color = self
                .a
                .cover
                .as_ref()
                .map(|c| rgb8(c.glow.0, c.glow.1, c.glow.2))
                .or_else(|| self.a.glow_color().map(|c| c.alpha(1.0)));
            if let Some(c) = color {
                let h = self.a.expanded_height();
                let base_boost = if self.a.glow_mode == 1 {
                    if m.active && m.playing { 0.36 } else { 0.22 }
                } else if m.active && m.playing {
                    0.20
                } else {
                    0.12
                };
                let boost = (base_boost * self.a.glow_strength_factor()).min(0.85);
                self.g.fill_radial_glow(Rect::new(CX - 210.0, 0.0, 420.0, h), cover.cx(), cover.cy(), 290.0, 190.0, c.alpha(boost));
            }
        }
        if let Some(bitmap) = &bitmap {
            let mask = self.g.rrect_geometry(cover, 18.0);
            self.g.push_mask(mask.as_ref(), 1.0);
            self.g.draw_bitmap(bitmap, cover);
            self.g.pop_layer();
            self.g.stroke_rrect(cover.inset(0.5), 17.5, argb(0x1FFFFFFF), 1.0, false);
        } else {
            self.g.fill_rrect(cover, 18.0, Paint::Solid(CARD));
            self.g.text("\u{E8D6}", cover, ts(34.0).icon().center().vcenter(), TEXT3);
        }

        let x = cover.right() + 16.0;
        let w = CONTENT_R - x;
        if m.active {
            let app = if m.app.is_empty() { "Wiedergabe".to_string() } else { m.app.clone() };
            let state = if m.playing { "Spielt" } else { "Pausiert" };
            self.g.text("\u{E8D6}", Rect::new(x, CONTENT_Y + 2.0, 14.0, 16.0), ts(10.0).icon(), accent());
            self.g.text(&format!("{app} · {state}"), Rect::new(x + 17.0, CONTENT_Y, w - 17.0, 16.0), ts(10.5).bold().trim(), TEXT2);
            let title = if m.title.is_empty() { "Unbekannter Titel".to_string() } else { m.title.clone() };
            self.g.text(&title, Rect::new(x, CONTENT_Y + 18.0, w, 26.0), ts(17.0).bold().trim(), WHITE);
            self.g.text(&m.artist, Rect::new(x, CONTENT_Y + 44.0, w, 18.0), ts(12.5).trim(), argb(0xD9FFFFFF));
            self.g.text(&m.album, Rect::new(x, CONTENT_Y + 63.0, w, 16.0), ts(11.0).trim(), TEXT3);
        } else {
            self.g.text("Keine Wiedergabe", Rect::new(x, CONTENT_Y + 14.0, w, 26.0), ts(17.0).bold().trim(), WHITE);
            self.g.text("Musik in Spotify, im Browser oder einem Player starten", Rect::new(x, CONTENT_Y + 40.0, w, 16.0), ts(11.0).trim(), TEXT2);
        }

        // Transport: previous, play/pause (big), next.
        let cy = CONTENT_Y + 104.0;
        let dim = |on: bool| if on { WHITE } else { TEXT2 };
        self.icon_button(Id::MediaPrev, Rect::new(x - 4.0, cy - 17.0, 34.0, 34.0), "\u{E892}", 13.0, dim(!m.active || m.can_prev), None);
        let play = if m.playing { "\u{E769}" } else { "\u{E768}" };
        self.icon_button(Id::MediaPlay, Rect::new(x + 36.0, cy - 21.0, 42.0, 42.0), play, 15.0, WHITE, Some(Paint::Grad));
        self.icon_button(Id::MediaNext, Rect::new(x + 84.0, cy - 17.0, 34.0, 34.0), "\u{E893}", 13.0, dim(!m.active || m.can_next), None);

        // Volume on the right of the transport row.
        let muted = self.a.tel.muted;
        let vol = format!("{} %", self.a.tel.volume.clamp(0, 100));
        self.g.text(if muted { "\u{E74F}" } else { "\u{E767}" }, Rect::new(CONTENT_R - 64.0, cy - 9.0, 16.0, 18.0), ts(12.0).icon().vcenter(), TEXT2);
        self.g.text(if muted { "Stumm" } else { &vol }, Rect::new(CONTENT_R - 46.0, cy - 9.0, 46.0, 18.0), ts(11.0).bold().right().vcenter(), TEXT2);

        // Timeline: elapsed, bar, total. A click on the bar jumps there.
        let bar = MEDIA_BAR;
        let track = Rect::new(bar.x, bar.cy() - 2.0, bar.w, 4.0);
        let (pos, dur) = (self.a.media_position(), m.duration_ms);
        if dur > 0 {
            let fraction = (pos as f32 / dur as f32).clamp(0.0, 1.0);
            self.bar(track, fraction, Paint::GradH);
            if m.can_seek {
                let hover = self.anim(Key::Hover(Id::MediaSeek), 0.0);
                self.g.fill_ellipse(track.x + track.w * fraction, track.cy(), 4.0 + 2.5 * hover, Paint::Solid(WHITE));
                self.add_hit(Id::MediaSeek, bar);
            }
            self.g.text(&format_ms(pos), Rect::new(CONTENT_X, bar.y, 44.0, bar.h), ts(10.5).vcenter(), TEXT2);
            self.g.text(&format_ms(dur), Rect::new(CONTENT_R - 44.0, bar.y, 44.0, bar.h), ts(10.5).right().vcenter(), TEXT2);
        } else {
            self.g.fill_rrect(track, 2.0, Paint::Solid(TRACK));
            let label = if m.active { "Keine Zeitleiste" } else { "–" };
            self.g.text(label, Rect::new(CONTENT_X, bar.y, 44.0, bar.h), ts(10.5).vcenter(), TEXT3);
        }
    }

    // ---- Tab 2: colour picker ----

    fn tab_color(&mut self) {
        let shown = self.a.shown_color;
        let swatch = Rect::new(CONTENT_X, CONTENT_Y, 88.0, 88.0);
        if self.a.picking {
            // Loupe: the screen pixels around the cursor, enlarged without smoothing.
            let mask = self.g.rrect_geometry(swatch, 22.0);
            self.g.push_mask(mask.as_ref(), 1.0);
            let cell = 88.0 / LOUPE as f32;
            for py in 0..LOUPE {
                for px in 0..LOUPE {
                    let c = self.a.loupe_pixels[py * LOUPE + px];
                    let color = rgb8((c >> 16) as u8, (c >> 8) as u8, c as u8);
                    self.g.fill_rect(Rect::new(swatch.x + px as f32 * cell, swatch.y + py as f32 * cell, cell + 0.5, cell + 0.5), Paint::Solid(color));
                }
            }
            self.g.pop_layer();
            self.g.stroke_rrect(Rect::new(swatch.cx() - 4.5, swatch.cy() - 4.5, 9.0, 9.0), 0.0, WHITE, 1.0, false);
        } else {
            self.g.fill_rrect(swatch, 22.0, Paint::Solid(rgb8(shown.0, shown.1, shown.2)));
        }
        self.g.stroke_rrect(swatch.inset(0.5), 21.5, argb(0x24FFFFFF), 1.0, false);

        let x = 192.0;
        let y0 = CONTENT_Y + (88.0 - 82.3) / 2.0;

        // HEX value: plain text button.
        let hex = to_hex(shown);
        let hex_style = ts(22.0).bold();
        let hex_w = self.g.measure(&hex, hex_style);
        let hex_rect = Rect::new(x, y0, hex_w + 2.0, line_height(22.0));
        let saved = self.g.alpha;
        if self.a.hot == Id::Hex {
            self.g.alpha = saved * 0.7;
        }
        self.g.text(&hex, Rect::new(x, y0, hex_w + 4.0, 34.0), hex_style, WHITE);
        self.g.alpha = saved;
        self.add_hit(Id::Hex, hex_rect);

        let label = if self.a.picking { "Abbrechen" } else { "Pipette" };
        self.label_button(Id::Pipette, CONTENT_R, y0 + 0.65, 28.0, 14.0, 0.0, label, ts(12.0).bold(), WHITE, Paint::Grad, Some("\u{EF3C}"), true);

        let chips_y = y0 + line_height(22.0) + 7.0;
        let rgb = self.chip(Id::Rgb, x, chips_y, 24.0, &format!("RGB {}", to_rgb(shown)));
        self.chip(Id::Hsl, rgb.right() + 6.0, chips_y, 24.0, &format!("HSL {}", to_hsl(shown)));

        let hint = self.a.color_hint.clone();
        self.g.text(&hint, Rect::new(x + 1.0, chips_y + 24.0 + 8.0, CONTENT_R - x - 1.0, 16.0), ts(10.5).trim(), TEXT3);

        // History dots along the bottom.
        let history = self.a.history.clone();
        for (i, c) in history.iter().enumerate() {
            let id = Id::History(i as u8);
            let r = Rect::new(CONTENT_X + i as f32 * 26.0, CONTENT_Y + CONTENT_H - 20.0, 20.0, 20.0);
            let saved = self.g.alpha;
            if i == 0 {
                let pop = self.anim(Key::PopScale(0), 1.0);
                self.g.alpha = saved * self.anim(Key::PopOpacity(0), 1.0);
                self.g.push_scale(r.cx(), r.cy(), pop, pop);
            }
            self.icon_button(id, r, "", 9.0, WHITE, Some(Paint::Solid(rgb8(c.0, c.1, c.2))));
            if i == 0 {
                self.g.pop_xf();
                self.g.alpha = saved;
            }
        }
    }

    // ---- Tab 2: file shelf ----

    fn tab_files(&mut self) {
        if self.a.share_open {
            self.share_panel();
            return;
        }
        if self.a.nearby_open {
            self.nearby_panel();
            return;
        }

        // Header row: info text, "Handy", "Leeren".
        let mut right = CONTENT_R;
        if !self.a.shelf.is_empty() {
            let st = ts(11.0).bold();
            let w = self.g.measure("Leeren", st).ceil();
            let r = Rect::new(right - w, CONTENT_Y, w, 18.0);
            let fade = if self.a.hot == Id::FilesClear { 0.7 } else { 1.0 };
            self.g.text("Leeren", Rect::new(r.x, r.y, w + 2.0, 18.0), st.vcenter(), accent().fade(fade));
            self.add_hit(Id::FilesClear, r);
            right -= w + 12.0;
        }
        {
            let st = ts(11.0).bold();
            let w = self.g.measure("Handy", st).ceil();
            let r = Rect::new(right - w - 15.0, CONTENT_Y, w + 15.0, 18.0);
            let fade = if self.a.hot == Id::Phone { 0.7 } else { 1.0 };
            self.g.text("\u{ED14}", Rect::new(r.x, r.y + 1.0, 12.0, 18.0), ts(11.0).icon().vcenter(), accent().fade(fade));
            self.g.text("Handy", Rect::new(r.x + 15.0, r.y, w + 2.0, 18.0), st.vcenter(), accent().fade(fade));
            self.add_hit(Id::Phone, r);
            right = r.x - 10.0;
        }
        if !self.a.shelf.is_empty() {
            let st = ts(11.0).bold();
            let w = self.g.measure("Senden", st).ceil();
            let r = Rect::new(right - w - 15.0, CONTENT_Y, w + 15.0, 18.0);
            let fade = if self.a.hot == Id::Nearby { 0.7 } else { 1.0 };
            self.g.text("\u{E72D}", Rect::new(r.x, r.y + 1.0, 12.0, 18.0), ts(11.0).icon().vcenter(), accent().fade(fade));
            self.g.text("Senden", Rect::new(r.x + 15.0, r.y, w + 2.0, 18.0), st.vcenter(), accent().fade(fade));
            self.add_hit(Id::Nearby, r);
            right = r.x - 10.0;
        }
        let info = self.a.files_info.clone();
        self.g.text(&info, Rect::new(CONTENT_X + 2.0, CONTENT_Y, right - CONTENT_X - 2.0, 18.0), ts(11.0).vcenter().trim(), TEXT2);

        let area = Rect::new(CONTENT_X, CONTENT_Y + 26.0, CONTENT_W, 98.0);
        if self.a.shelf.is_empty() {
            self.g.fill_rrect(area, 18.0, Paint::Solid(argb(0x08FFFFFF)));
            self.g.stroke_rrect(area.inset(0.5), 17.5, argb(0x33FFFFFF), 1.0, true);
            let y = area.y + (area.h - 57.0) / 2.0;
            self.g.text("\u{E896}", Rect::new(area.x, y, area.w, 18.0), ts(18.0).icon().center().vcenter(), TEXT2);
            self.g.text("Dateien hier ablegen", Rect::new(area.x, y + 25.0, area.w, 18.0), ts(12.0).bold().center(), WHITE);
            self.g.text("Wieder herausziehen oder als Link im WLAN teilen", Rect::new(area.x, y + 43.0, area.w, 16.0), ts(10.5).center(), TEXT3);
            return;
        }

        // Chips scroll horizontally inside the area.
        self.g.push_clip(area);
        let scroll = self.a.files_scroll;
        for index in 0..self.a.shelf.len() {
            let chip = Rect::new(area.x + index as f32 * CHIP_PITCH - scroll, area.y, 82.0, area.h);
            if chip.right() < area.x || chip.x > area.right() {
                continue;
            }
            self.file_chip(index, chip, area);
        }
        self.g.pop_clip();
    }

    fn file_chip(&mut self, index: usize, r: Rect, area: Rect) {
        let id = self.a.shelf[index].id;
        let name = self.a.shelf[index].name.clone();

        // The icon bitmap lives on the GPU device; build it on first use.
        if self.a.shelf[index].bitmap.is_none() {
            if let Some(icon) = &self.a.shelf[index].icon {
                self.a.shelf[index].bitmap = self.g.create_bitmap(icon.size, icon.size, &icon.pixels);
            }
        }
        let bitmap = self.a.shelf[index].bitmap.clone();

        let pop = self.anim(Key::PopScale(id), 1.0);
        let saved = self.g.alpha;
        self.g.alpha = saved * self.anim(Key::PopOpacity(id), 1.0);
        self.g.push_scale(r.cx(), r.cy(), pop, pop);

        self.g.fill_rrect(r, 16.0, Paint::Solid(CARD));
        let y = r.y + 4.0 + (r.h - 4.0 - 53.0) / 2.0;
        if let Some(bitmap) = &bitmap {
            self.g.draw_bitmap(bitmap, Rect::new(r.cx() - 16.0, y, 32.0, 32.0));
        }
        self.g.text(&name, Rect::new(r.x + 7.0, y + 39.0, r.w - 14.0, 16.0), ts(10.5).center().trim(), TEXT2);
        self.add_hit(Id::Chip(id), r.intersect(&area));

        // Hover actions: send nearby + share link + remove.
        let actions = self.anim(Key::ChipActions(id), 0.0);
        let remove = Rect::new(r.right() - 5.0 - 20.0, r.y + 5.0, 20.0, 20.0);
        let link = Rect::new(remove.x - 3.0 - 20.0, r.y + 5.0, 20.0, 20.0);
        let send = Rect::new(link.x - 3.0 - 20.0, r.y + 5.0, 20.0, 20.0);
        let chip_alpha = self.g.alpha;
        self.g.alpha = chip_alpha * actions;
        let was_hit = self.hit;
        // Hidden buttons must not take the click that starts a drag.
        self.hit = was_hit && actions > 0.05 && area.contains(remove.cx(), remove.cy());
        let mini = Paint::Solid(argb(0xE6242426));
        if actions > 0.004 {
            if !self.a.shelf[index].is_dir {
                self.icon_button(Id::ChipSend(id), send, "\u{E72D}", 9.0, WHITE, Some(mini));
            }
            self.icon_button(Id::ChipLink(id), link, "\u{E71B}", 9.0, WHITE, Some(mini));
            self.icon_button(Id::ChipRemove(id), remove, "\u{E711}", 9.0, WHITE, Some(mini));
        }
        self.hit = was_hit;

        self.g.pop_xf();
        self.g.alpha = saved;
    }

    /// Phone share: QR code for the hub page, covers the shelf while open.
    fn share_panel(&mut self) {
        let saved = self.g.alpha;
        self.g.alpha = saved * self.anim(Key::SharePanel, 1.0);

        let panel = Rect::new(CONTENT_X, CONTENT_Y, CONTENT_W, CONTENT_H);
        self.g.fill_rect(panel, Paint::Solid(BLACK));
        let white_box = Rect::new(CONTENT_X, CONTENT_Y + 2.0, 120.0, 120.0);
        self.g.fill_rrect(white_box, 14.0, Paint::Solid(WHITE));

        if let Some(qr) = &self.a.qr {
            // Whole pixels per module keep the code crisp.
            let n = qr.size;
            let module = (110 / n).max(1) as f32;
            let origin_x = white_box.cx() - n as f32 * module / 2.0;
            let origin_y = white_box.cy() - n as f32 * module / 2.0;
            for y in 0..n {
                // Merge runs of dark modules into one rectangle.
                let mut x = 0;
                while x < n {
                    if !qr.dark(x, y) {
                        x += 1;
                        continue;
                    }
                    let start = x;
                    while x < n && qr.dark(x, y) {
                        x += 1;
                    }
                    self.g.fill_rect(
                        Rect::new(origin_x + start as f32 * module, origin_y + y as f32 * module, (x - start) as f32 * module, module),
                        Paint::Solid(BLACK),
                    );
                }
            }
        }

        let x = 224.0;
        let y0 = CONTENT_Y + (CONTENT_H - 93.6) / 2.0;
        self.g.text("Mit dem Handy teilen", Rect::new(x, y0, CONTENT_R - x, 24.0), ts(15.0).bold(), WHITE);
        self.g.text("QR-Code scannen · gleiches WLAN", Rect::new(x, y0 + 21.0, CONTENT_R - x, 18.0), ts(11.0), TEXT2);
        let hint = self.a.share_hint.clone();
        self.g.text(&hint, Rect::new(x, y0 + 41.6, CONTENT_R - x, 16.0), ts(10.5).trim(), TEXT3);

        let by = y0 + 65.6;
        let copy = self.label_button(Id::ShareCopy, x, by, 28.0, 14.0, 0.0, "Link kopieren", ts(12.0).bold(), WHITE, Paint::Grad, None, false);
        let folder = self.chip(Id::ShareFolder, copy.right() + 6.0, by, 28.0, "Ordner");
        self.chip(Id::ShareClose, folder.right() + 6.0, by, 28.0, "Schließen");

        self.g.alpha = saved;
    }

    /// Nearby share: PCs on the network that run the island, covers the shelf while open.
    fn nearby_panel(&mut self) {
        let saved = self.g.alpha;
        self.g.alpha = saved * self.anim(Key::SharePanel, 1.0);

        let panel = Rect::new(CONTENT_X, CONTENT_Y, CONTENT_W, CONTENT_H);
        self.g.fill_rect(panel, Paint::Solid(BLACK));

        let close = self.label_button(Id::NearbyClose, CONTENT_R, CONTENT_Y, 24.0, 10.0, 0.0, "Schließen", ts(11.0), CHIP_TEXT, Paint::Solid(CARD), None, true);
        self.g.text("An PC in der Nähe senden", Rect::new(CONTENT_X, CONTENT_Y, close.x - CONTENT_X - 8.0, 22.0), ts(14.0).bold().trim(), WHITE);
        let count = self.a.nearby_files.len();
        let what = if count == 1 {
            self.a.nearby_files[0].rsplit(['\\', '/']).next().unwrap_or("").to_string()
        } else {
            format!("{count} Dateien")
        };
        let sub = format!("{what} · {}", format_size(self.a.nearby_bytes));
        self.g.text(&sub, Rect::new(CONTENT_X, CONTENT_Y + 20.0, close.x - CONTENT_X - 8.0, 16.0), ts(11.0).trim(), TEXT2);

        let row_y = CONTENT_Y + 44.0;
        if self.a.peers.is_empty() {
            let area = Rect::new(CONTENT_X, row_y, CONTENT_W, 62.0);
            self.g.fill_rrect(area, 16.0, Paint::Solid(argb(0x08FFFFFF)));
            self.g.stroke_rrect(area.inset(0.5), 15.5, argb(0x33FFFFFF), 1.0, true);
            self.g.text("Suche nach PCs …", Rect::new(area.x, area.y + 13.0, area.w, 18.0), ts(12.0).bold().center(), WHITE);
            self.g.text("Island muss dort laufen und unter WLAN-Tab „Nearby“ sichtbar sein", Rect::new(area.x + 10.0, area.y + 32.0, area.w - 20.0, 16.0), ts(10.5).center().trim(), TEXT3);
        } else {
            let sending = self.a.sending.clone();
            for i in 0..self.a.peers.len().min(MAX_PEERS) {
                let name = self.a.peers[i].name.clone();
                let x = CONTENT_X + i as f32 * 76.0;
                let bubble = Rect::new(x + 15.0, row_y, 46.0, 46.0);
                let pop = self.anim(Key::PopScale(PEER_POP + i as u32), 1.0);
                self.g.push_scale(bubble.cx(), bubble.cy(), pop, pop);
                let initials: String = name.chars().filter(|c| c.is_alphanumeric()).take(2).collect::<String>().to_uppercase();
                self.icon_button(Id::Peer(i as u8), bubble, "", 9.0, WHITE, Some(Paint::Grad));
                self.g.text(&initials, bubble, ts(14.0).bold().center().vcenter(), WHITE);
                if let Some((to, percent)) = &sending {
                    if *to == name {
                        self.g.stroke_ellipse(bubble.cx(), bubble.cy(), 26.0, TRACK, 3.0);
                        self.g.stroke_arc(bubble.cx(), bubble.cy(), 26.0, (*percent as f32 / 100.0).max(0.02), 3.0, Paint::Solid(WHITE));
                    }
                }
                self.g.pop_xf();
                self.g.text(&name, Rect::new(x, row_y + 50.0, 76.0, 16.0), ts(10.5).center().trim(), TEXT2);
            }
        }

        let hint = self.a.nearby_hint.clone();
        if !hint.is_empty() {
            self.g.text(&hint, Rect::new(CONTENT_X, CONTENT_Y + CONTENT_H - 12.0, CONTENT_W, 14.0), ts(10.5).trim(), TEXT3);
        }
        self.g.alpha = saved;
    }

    // ---- Tab 4: Wi-Fi, Bluetooth, nearby ----

    /// Big toggle tile: icon, label, state line. Filled with the accent gradient while on.
    #[allow(clippy::too_many_arguments)]
    fn toggle_tile(&mut self, id: Id, r: Rect, glyph: &str, label: &str, status: &str, on: bool) {
        let hover = self.anim(Key::Hover(id), 0.0);
        let press = self.anim(Key::Press(id), 1.0);
        self.g.push_scale(r.cx(), r.cy(), press, press);
        self.g.fill_rrect(r, 18.0, if on { Paint::Grad } else { Paint::Solid(CARD) });
        if hover > 0.0 {
            self.g.fill_rrect(r, 18.0, Paint::Solid(HOVER.fade(hover)));
        }
        let icon = Rect::new(r.x + 10.0, r.cy() - 17.0, 34.0, 34.0);
        self.g.fill_ellipse(icon.cx(), icon.cy(), 17.0, Paint::Solid(if on { argb(0x40FFFFFF) } else { argb(0x1AFFFFFF) }));
        self.g.text(glyph, icon, ts(14.0).icon().center().vcenter(), if on { WHITE } else { TEXT2 });
        let tx = icon.right() + 9.0;
        let tw = r.right() - tx - 8.0;
        self.g.text(label, Rect::new(tx, r.cy() - 17.0, tw, 18.0), ts(12.5).bold().trim(), WHITE);
        self.g.text(status, Rect::new(tx, r.cy() + 1.0, tw, 16.0), ts(10.5).trim(), if on { argb(0xD9FFFFFF) } else { TEXT2 });
        self.g.pop_xf();
        self.add_hit(id, r);
    }

    fn tab_controls(&mut self) {
        let radio_text = |status: RadioStatus, on_text: String| -> (String, bool) {
            match status {
                RadioStatus::On => (on_text, true),
                RadioStatus::Off => ("Aus".to_string(), false),
                RadioStatus::Disabled => ("Gesperrt".to_string(), false),
                RadioStatus::Missing => ("Nicht vorhanden".to_string(), false),
                RadioStatus::Unknown => ("…".to_string(), false),
            }
        };
        let wifi_on_text = self.a.wifi.as_ref().map(|(name, _)| name.clone()).unwrap_or_else(|| "An".to_string());
        let (wifi_text, wifi_on) = radio_text(self.a.radio_wifi, wifi_on_text);
        let bt_names = self.a.bt_connected_names();
        let bt_on_text = if bt_names.is_empty() { "An".to_string() } else { format!("{} verbunden", bt_names.len()) };
        let (bt_text, bt_on) = radio_text(self.a.radio_bt, bt_on_text);
        let nearby = self.a.nearby_visible;

        let y = CONTENT_Y;
        self.toggle_tile(Id::Toggle(0), Rect::new(88.0, y, 124.0, 64.0), "\u{E701}", "WLAN", &wifi_text, wifi_on);
        self.toggle_tile(Id::Toggle(1), Rect::new(218.0, y, 124.0, 64.0), "\u{E702}", "Bluetooth", &bt_text, bt_on);
        self.toggle_tile(Id::Toggle(2), Rect::new(348.0, y, 124.0, 64.0), "\u{E72D}", "Nearby", if nearby { "Sichtbar" } else { "Unsichtbar" }, nearby);

        // Network card: name, address, signal bars.
        let card_y = y + 72.0;
        let left = Rect::new(88.0, card_y, 189.0, 52.0);
        self.g.fill_rrect(left, 16.0, Paint::Solid(CARD));
        let label = if self.a.local_ip.is_empty() { "Netzwerk".to_string() } else { format!("Netzwerk · {}", self.a.local_ip) };
        self.g.text(&label, Rect::new(left.x + 13.0, left.y + 8.0, left.w - 26.0, 16.0), ts(10.5).trim(), TEXT2);
        let (net, bars) = match &self.a.wifi {
            Some((name, bars)) => (name.clone(), Some(*bars)),
            None => (self.a.net_name.clone(), None),
        };
        let bars_w = if bars.is_some() { 22.0 } else { 0.0 };
        self.g.text(&net, Rect::new(left.x + 13.0, left.y + 25.0, left.w - 26.0 - bars_w, 20.0), ts(12.5).bold().trim(), WHITE);
        if let Some(bars) = bars {
            // Four bars, lit by signal strength (0..5 from Windows).
            let level = ((bars as f32) * 4.0 / 5.0).round() as i32;
            for i in 0..4 {
                let h = 4.0 + i as f32 * 3.0;
                let bar = Rect::new(left.right() - 13.0 - 16.0 + i as f32 * 4.5, left.y + 41.0 - h, 3.0, h);
                self.g.fill_rrect(bar, 1.5, Paint::Solid(if i < level { WHITE } else { TRACK }));
            }
        }

        // Bluetooth card: connected devices.
        let right = Rect::new(283.0, card_y, 189.0, 52.0);
        self.g.fill_rrect(right, 16.0, Paint::Solid(CARD));
        self.g.text("Bluetooth-Geräte", Rect::new(right.x + 13.0, right.y + 8.0, right.w - 26.0, 16.0), ts(10.5).trim(), TEXT2);
        let devices = if bt_names.is_empty() { "Keine verbunden".to_string() } else { bt_names.join(", ") };
        self.g.text(&devices, Rect::new(right.x + 13.0, right.y + 25.0, right.w - 26.0, 20.0), ts(12.5).bold().trim(), if bt_names.is_empty() { TEXT2 } else { WHITE });
    }

    // ---- Tab 3: focus timer ----

    fn tab_focus(&mut self) {
        // Progress ring: arc from 12 o'clock, clockwise, for the remaining share.
        let (ring_cx, ring_cy, radius) = (146.0, 120.0, 48.0);
        self.g.stroke_ellipse(ring_cx, ring_cy, radius, TRACK, 6.0);
        let fraction = if self.a.focus_total > 0 { self.a.focus_remaining as f32 / self.a.focus_total as f32 } else { 0.0 };
        self.g.stroke_arc(ring_cx, ring_cy, radius, fraction, 6.0, Paint::Grad);
        let time = format_seconds(self.a.focus_remaining);
        self.g.text(&time, Rect::new(92.0, 66.0, 108.0, 108.0), ts(22.0).bold().center().vcenter(), WHITE);

        let x = 224.0;
        let y0 = CONTENT_Y + (CONTENT_H - 107.6) / 2.0;
        let (running, remaining, total) = (self.a.focus_running, self.a.focus_remaining, self.a.focus_total);
        let status = if running {
            "Läuft".to_string()
        } else if remaining < total {
            "Pausiert".to_string()
        } else {
            format!("{} Minuten", total / 60)
        };
        self.g.text("Fokus", Rect::new(x, y0, 200.0, 24.0), ts(15.0).bold(), WHITE);
        self.g.text(&status, Rect::new(x, y0 + 21.0, 200.0, 18.0), ts(11.0), TEXT2);

        let presets_y = y0 + 45.6;
        let mut px = x;
        for minutes in [15u8, 25, 50] {
            let r = self.chip(Id::Preset(minutes), px, presets_y, 24.0, &format!("{minutes} min"));
            px = r.right() + 6.0;
        }

        let controls_y = presets_y + 34.0;
        let label = if running {
            "Pause"
        } else if remaining < total {
            "Weiter"
        } else {
            "Start"
        };
        let toggle = self.label_button(Id::FocusToggle, x, controls_y, 28.0, 14.0, 78.0, label, ts(12.0).bold(), WHITE, Paint::Grad, None, false);
        self.icon_button(Id::FocusReset, Rect::new(toggle.right() + 6.0, controls_y, 28.0, 28.0), "\u{E72C}", 12.0, WHITE, Some(Paint::Solid(CARD)));
    }

    // ---- Settings window ----

    fn section_at(&mut self, label: &str, x: f32, y: f32) {
        self.g.text(label, Rect::new(x + 2.0, y, 240.0, 16.0), ts(10.5).bold(), TEXT3);
    }

    fn settings_page(&mut self) {
        const X: f32 = 24.0;
        const W: f32 = SW_W - 48.0;

        // Title bar: drag area (see settings.rs WM_NCHITTEST), minimise, close.
        self.g.text("\u{E713}", Rect::new(22.0, 0.0, 18.0, TITLE_H), ts(14.0).icon().vcenter(), accent());
        self.g.text("Einstellungen", Rect::new(48.0, 0.0, 220.0, TITLE_H), ts(15.0).bold().vcenter(), WHITE);
        self.icon_button(Id::SettingsMin, Rect::new(SW_W - 80.0, 10.0, 28.0, 28.0), "\u{E921}", 10.0, TEXT2, None);
        self.icon_button(Id::SettingsClose, Rect::new(SW_W - 46.0, 10.0, 28.0, 28.0), "\u{E8BB}", 10.0, TEXT2, None);
        self.g.fill_rect(Rect::new(0.0, TITLE_H, SW_W, 1.0), Paint::Solid(argb(0x14FFFFFF)));

        // Segmented tab bar across the top of the settings window.
        let bar = Rect::new(X, 60.0, W, 36.0);
        self.g.fill_rrect(bar, 18.0, Paint::Solid(CARD));
        let tab_w = (W - 6.0) / SETTINGS_TAB_COUNT as f32;
        let highlight = self.anim(Key::SettingsTabHighlight, self.a.settings_tab as f32).clamp(0.0, (SETTINGS_TAB_COUNT - 1) as f32);
        self.g.fill_rrect(Rect::new(bar.x + 3.0 + highlight * tab_w, bar.y + 3.0, tab_w, 30.0), 15.0, Paint::Grad);
        const SETTINGS_TABS: [(&str, &str); SETTINGS_TAB_COUNT] = [
            ("\u{E790}", "Darstellung"),
            ("\u{E713}", "Verhalten"),
            ("\u{E753}", "Wetter"),
            ("\u{E765}", "Kürzel"),
        ];
        for (i, (glyph, label)) in SETTINGS_TABS.iter().enumerate() {
            let id = Id::SettingsTab(i as u8);
            let r = Rect::new(bar.x + 3.0 + i as f32 * tab_w, bar.y + 3.0, tab_w, 30.0);
            let hover = self.anim(Key::Hover(id), 0.0);
            if hover > 0.0 && i != self.a.settings_tab {
                self.g.fill_rrect(r, 15.0, Paint::Solid(HOVER.fade(hover)));
            }
            let st = ts(11.5).bold();
            let tw = self.g.measure(label, st);
            let icon_w = 13.0 + 5.0;
            let content_w = icon_w + tw;
            let tx = r.x + (r.w - content_w) / 2.0;
            let fg = if i == self.a.settings_tab { WHITE } else { TEXT2 };
            self.g.text(glyph, Rect::new(tx, r.y + 1.0, 13.0, r.h), ts(11.5).icon().vcenter(), fg);
            self.g.text(label, Rect::new(tx + icon_w, r.y, tw + 2.0, r.h), st.vcenter(), fg);
            self.add_hit(id, r);
        }

        // Active tab content with crossfade + horizontal shift.
        let saved_alpha = self.g.alpha;
        for i in 0..SETTINGS_TAB_COUNT {
            let k = i as u8;
            let opacity = self.anim(Key::SettingsTabOpacity(k), if i == self.a.settings_tab { 1.0 } else { 0.0 });
            if opacity <= 0.004 {
                continue;
            }
            let shift = self.anim(Key::SettingsTabShift(k), 0.0);
            self.hit = i == self.a.settings_tab;
            self.g.alpha = saved_alpha * opacity;
            self.g.push_translate(shift, 0.0);
            match i {
                0 => self.settings_tab_appearance(),
                1 => self.settings_tab_behavior(),
                2 => self.settings_tab_weather(),
                _ => self.settings_tab_shortcuts(),
            }
            self.g.pop_xf();
        }
        self.g.alpha = saved_alpha;
        self.hit = true;

        // Persistent footer actions.
        self.g.fill_rect(Rect::new(0.0, 444.0, SW_W, 1.0), Paint::Solid(argb(0x14FFFFFF)));
        let clear = self.label_button(Id::ClearNotices, X, 458.0, 28.0, 14.0, 0.0, "Meldungen leeren", ts(11.0), CHIP_TEXT, Paint::Solid(CARD), Some("\u{E74D}"), false);
        self.label_button(Id::ResetSettings, clear.right() + 8.0, 458.0, 28.0, 14.0, 0.0, "Standard", ts(11.0), CHIP_TEXT, Paint::Solid(CARD), Some("\u{E72C}"), false);
        self.g.text("Dynamic Island", Rect::new(X, 458.0, W, 28.0), ts(10.5).right().vcenter(), TEXT3);
    }

    fn settings_tab_appearance(&mut self) {
        const X: f32 = 24.0;
        const W: f32 = SW_W - 48.0;
        let half = (W - 8.0) / 2.0;

        // Accent colour: one swatch per preset.
        self.section_at("AKZENTFARBE", X, 112.0);
        let current = accent_index();
        let name = ACCENTS[current as usize].0;
        self.g.text(name, Rect::new(X + W - 150.0, 112.0, 150.0, 16.0), ts(10.5).right(), TEXT2);
        let n = ACCENTS.len();
        let gap = (W - n as f32 * 30.0) / (n as f32 - 1.0);
        for (i, (_, a, _)) in ACCENTS.iter().enumerate() {
            let r = Rect::new(X + i as f32 * (30.0 + gap), 132.0, 30.0, 30.0);
            let selected = i as u8 == current;
            if selected {
                self.g.stroke_ellipse(r.cx(), r.cy(), 18.0, WHITE, 2.0);
            }
            self.icon_button(Id::Accent(i as u8), r, if selected { "\u{E73E}" } else { "" }, 11.0, WHITE, Some(Paint::Solid(argb(*a))));
        }

        // Inner glow: switch + mode + intensity.
        self.section_at("INNEN-LEUCHTEN", X, 178.0);
        let (glow_label, glow_desc) = crate::island::SWITCHES[SWITCH_GLOW as usize];
        self.switch_card(SWITCH_GLOW, Rect::new(X, 196.0, W, 38.0), glow_label, glow_desc);
        self.g.text("Leucht-Modus", Rect::new(X + 2.0, 242.0, half, 16.0), ts(11.0), TEXT2);
        self.g.text("Intensität", Rect::new(X + half + 10.0, 242.0, half, 16.0), ts(11.0), TEXT2);
        self.segmented(2, Rect::new(X, 260.0, half, 30.0), ["Bei Musik", "Nur Medien", "Immer"]);
        self.segmented(3, Rect::new(X + half + 8.0, 260.0, half, 30.0), ["Dezent", "Normal", "Kräftig"]);

        // Compact pill items.
        self.section_at("KOMPAKT-PILL", X, 306.0);
        let (l4, d4) = crate::island::SWITCHES[4];
        let (l3, d3) = crate::island::SWITCHES[3];
        let (l5, d5) = crate::island::SWITCHES[5];
        self.switch_card(4, Rect::new(X, 324.0, half, 38.0), l4, d4);
        self.switch_card(3, Rect::new(X + half + 8.0, 324.0, half, 38.0), l3, d3);
        self.switch_card(5, Rect::new(X, 368.0, W, 38.0), l5, d5);
    }

    fn settings_tab_behavior(&mut self) {
        const X: f32 = 24.0;
        const W: f32 = SW_W - 48.0;
        let half = (W - 8.0) / 2.0;

        self.section_at("ZEITVERHALTEN", X, 112.0);
        self.g.text("Öffnen nach", Rect::new(X + 2.0, 130.0, half, 16.0), ts(11.0), TEXT2);
        self.g.text("Anzeigedauer Meldungen", Rect::new(X + half + 10.0, 130.0, half, 16.0), ts(11.0), TEXT2);
        self.segmented(0, Rect::new(X, 148.0, half, 30.0), ["Sofort", "Normal", "Langsam"]);
        self.segmented(1, Rect::new(X + half + 8.0, 148.0, half, 30.0), ["Kurz", "Normal", "Lang"]);

        self.section_at("BEDIENUNG & MELDUNGEN", X, 198.0);
        for (row, idx) in [0usize, 1, 2].into_iter().enumerate() {
            let (label, desc) = crate::island::SWITCHES[idx];
            let r = Rect::new(X, 216.0 + row as f32 * 48.0, W, 40.0);
            self.switch_card(idx as u8, r, label, desc);
        }
    }

    fn settings_tab_weather(&mut self) {
        const X: f32 = 24.0;
        const W: f32 = SW_W - 48.0;

        self.section_at("STANDORT", X, 112.0);
        let field = Rect::new(X, 130.0, W, 36.0);
        let focus = self.a.city_focus;
        let hover = self.anim(Key::Hover(Id::CityInput), 0.0);
        self.g.fill_rrect(field, 18.0, Paint::Solid(if focus { argb(0x1FFFFFFF) } else { CARD }));
        if !focus && hover > 0.0 {
            self.g.fill_rrect(field, 18.0, Paint::Solid(HOVER.fade(hover * 0.6)));
        }
        self.g.stroke_rrect(field.inset(0.5), 17.5, if focus { accent() } else { argb(0x1AFFFFFF) }, 1.0, false);
        self.add_hit(Id::CityInput, field);
        self.g.text("\u{E707}", Rect::new(field.x + 14.0, field.y, 14.0, field.h), ts(12.0).icon().vcenter(), if focus { accent() } else { TEXT2 });
        let area = Rect::new(field.x + 36.0, field.y, field.w - 50.0, field.h);
        let city = self.a.city_input.clone();
        let st = ts(12.5);
        let caret_x = if city.is_empty() {
            self.g.text("Leer = Windows-Standort · oder Stadt, z. B. München", area, st.vcenter().trim(), TEXT3);
            area.x
        } else {
            let tw = self.g.measure(&city, st);
            self.g.text(&city, Rect::new(area.x, area.y, tw + 6.0, area.h), st.vcenter(), WHITE);
            area.x + tw
        };
        if focus && self.a.caret_on {
            self.g.fill_rect(Rect::new(caret_x + 1.0, field.cy() - 8.0, 1.5, 16.0), Paint::Solid(accent()));
        }
        let hint = if focus { "Enter speichert · Esc verwirft" } else { "Ins Feld klicken, Stadt eingeben und mit Enter bestätigen" };
        self.g.text(hint, Rect::new(X + 4.0, 172.0, W - 8.0, 16.0), ts(10.5).trim(), TEXT3);

        self.section_at("AKTUELLES WETTER", X, 204.0);
        let card = Rect::new(X, 222.0, W, 56.0);
        self.g.fill_rrect(card, 14.0, Paint::Solid(CARD));
        if self.a.weather_loading && self.a.weather.is_none() {
            self.g.text("Lade Wetter …", Rect::new(card.x + 16.0, card.y, card.w - 32.0, card.h), ts(12.0).vcenter().trim(), TEXT2);
        } else if let Some(w) = self.a.weather.clone() {
            self.g.text(weather::glyph(&w.icon), Rect::new(card.x + 16.0, card.y + 1.0, 26.0, card.h), ts(20.0).icon().vcenter(), accent());
            let top = format!("{}° · {} · {}", w.temp, w.description, w.city);
            let sub = format!("Gefühlt {}° · {} km/h Wind · Stand {}", w.feels, w.wind, w.time);
            self.g.text(&top, Rect::new(card.x + 50.0, card.y + 9.0, card.w - 64.0, 20.0), ts(12.5).bold().trim(), WHITE);
            self.g.text(&sub, Rect::new(card.x + 50.0, card.y + 29.0, card.w - 64.0, 18.0), ts(10.5).trim(), TEXT2);
        } else if let Some(e) = self.a.weather_error.clone() {
            self.g.text(&e.text(), Rect::new(card.x + 16.0, card.y, card.w - 32.0, card.h), ts(11.5).vcenter().trim(), BELL);
        } else {
            self.g.text("Keine Wetterdaten geladen", Rect::new(card.x + 16.0, card.y, card.w - 32.0, card.h), ts(11.5).vcenter().trim(), TEXT3);
        }

        self.section_at("ANZEIGE", X, 296.0);
        let (l5, d5) = crate::island::SWITCHES[5];
        self.switch_card(5, Rect::new(X, 314.0, W, 40.0), l5, d5);
    }

    fn settings_tab_shortcuts(&mut self) {
        const X: f32 = 24.0;
        const W: f32 = SW_W - 48.0;

        self.section_at("TASTENKÜRZEL", X, 112.0);
        let (label, desc) = crate::island::SWITCHES[SWITCH_HOTKEY as usize];
        self.switch_card(SWITCH_HOTKEY, Rect::new(X, 130.0, W, 40.0), label, desc);

        self.section_at("KOMBINATION", X, 188.0);
        self.g.text("Tastenkombination", Rect::new(X + 2.0, 208.0, 160.0, 34.0), ts(11.5).vcenter(), TEXT2);
        let rec = Rect::new(X + W - 240.0, 208.0, 240.0, 34.0);
        let recording = self.a.hotkey_recording;
        let hover = self.anim(Key::Hover(Id::HotkeyRecord), 0.0);
        let press = self.anim(Key::Press(Id::HotkeyRecord), 1.0);
        self.g.push_scale(rec.cx(), rec.cy(), press, press);
        self.g.fill_rrect(rec, 17.0, Paint::Solid(if recording { argb(0x1FFFFFFF) } else { CARD }));
        if hover > 0.0 {
            self.g.fill_rrect(rec, 17.0, Paint::Solid(HOVER.fade(hover)));
        }
        self.g.stroke_rrect(rec.inset(0.5), 16.5, if recording { accent() } else { argb(0x1AFFFFFF) }, 1.0, false);
        let keys = if recording { "Tasten drücken …".to_string() } else { hotkey_text(self.a.hotkey_mods, self.a.hotkey_vk) };
        self.g.text("\u{E765}", Rect::new(rec.x + 14.0, rec.y, 14.0, rec.h), ts(12.0).icon().vcenter(), if recording { accent() } else { TEXT2 });
        self.g.text(&keys, Rect::new(rec.x + 34.0, rec.y, rec.w - 46.0, rec.h), ts(12.0).bold().vcenter().trim(), if recording { accent() } else { WHITE });
        self.g.pop_xf();
        self.add_hit(Id::HotkeyRecord, rec);
        let hint_r = Rect::new(X + 4.0, 250.0, W - 8.0, 16.0);
        if let Some(problem) = self.a.hotkey_problem.clone() {
            self.g.text(&problem, hint_r, ts(10.5).trim(), BELL);
        } else if recording {
            self.g.text("Neue Kombination drücken · Esc bricht ab", hint_r, ts(10.5).trim(), TEXT3);
        } else {
            self.g.text("Feld anklicken, um die Kombination zu ändern · auch im Tray-Menü ein-/ausblendbar", hint_r, ts(10.5).trim(), TEXT3);
        }
    }

    fn switch_card(&mut self, i: u8, r: Rect, label: &str, desc: &str) {
        let id = Id::Switch(i);
        let hover = self.anim(Key::Hover(id), 0.0);
        let press = self.anim(Key::Press(id), 1.0);
        let on = self.a.switch_value(i);
        let pos = self.anim(Key::SwitchPos(i), if on { 1.0 } else { 0.0 }).clamp(0.0, 1.0);
        self.g.push_scale(r.cx(), r.cy(), press, press);
        self.g.fill_rrect(r, 12.0, Paint::Solid(CARD));
        if hover > 0.0 {
            self.g.fill_rrect(r, 12.0, Paint::Solid(HOVER.fade(hover)));
        }
        let tw = r.w - 12.0 - 52.0;
        self.g.text(label, Rect::new(r.x + 12.0, r.y + 3.0, tw, 17.0), ts(11.5).bold().trim(), WHITE);
        self.g.text(desc, Rect::new(r.x + 12.0, r.y + 19.0, tw, 15.0), ts(10.0).trim(), TEXT3);

        // Track crossfades from grey to the accent gradient while the knob slides.
        let track = Rect::new(r.right() - 12.0 - 34.0, r.cy() - 10.0, 34.0, 20.0);
        self.g.fill_rrect(track, 10.0, Paint::Solid(TRACK));
        let saved = self.g.alpha;
        self.g.alpha = saved * pos;
        self.g.fill_rrect(track, 10.0, Paint::Grad);
        self.g.alpha = saved;
        let knob_x = track.x + 10.0 + pos * 14.0;
        self.g.fill_ellipse(knob_x, track.cy(), 8.0, Paint::Solid(WHITE));
        self.g.pop_xf();
        self.add_hit(id, r);
    }

    fn segmented(&mut self, control: u8, r: Rect, options: [&str; 3]) {
        self.g.fill_rrect(r, 14.0, Paint::Solid(CARD));
        let value = self.a.segment_value(control);
        let seg_w = (r.w - 4.0) / 3.0;
        let pos = self.anim(Key::SegPos(control), value as f32).clamp(0.0, 2.0);
        self.g.fill_rrect(Rect::new(r.x + 2.0 + pos * seg_w, r.y + 2.0, seg_w, r.h - 4.0), 12.0, Paint::Grad);
        for (o, label) in options.iter().enumerate() {
            let id = Id::Seg(control, o as u8);
            let seg = Rect::new(r.x + 2.0 + o as f32 * seg_w, r.y + 2.0, seg_w, r.h - 4.0);
            let hover = self.anim(Key::Hover(id), 0.0);
            if hover > 0.0 && o as u8 != value {
                self.g.fill_rrect(seg, 12.0, Paint::Solid(HOVER.fade(hover)));
            }
            let color = if o as u8 == value { WHITE } else { TEXT2 };
            self.g.text(label, seg, ts(11.5).bold().center().vcenter(), color);
            self.add_hit(id, seg);
        }
    }

    // ---- Tab 4: reminders ----

    fn tab_reminders(&mut self) {
        let field = Rect::new(CONTENT_X, CONTENT_Y, CONTENT_W, 36.0);
        let focus = self.a.input_focus;
        let hover = self.anim(Key::Hover(Id::ReminderInput), 0.0);
        self.g.fill_rrect(field, 18.0, Paint::Solid(if focus { argb(0x1FFFFFFF) } else { CARD }));
        if !focus && hover > 0.0 {
            self.g.fill_rrect(field, 18.0, Paint::Solid(HOVER.fade(hover * 0.6)));
        }
        self.g.stroke_rrect(field.inset(0.5), 17.5, if focus { accent() } else { argb(0x1AFFFFFF) }, 1.0, false);
        self.add_hit(Id::ReminderInput, field);
        self.g.text("\u{E710}", Rect::new(field.x + 14.0, field.y, 14.0, field.h), ts(12.0).icon().vcenter(), if focus { accent() } else { TEXT2 });

        let input = self.a.input.clone();
        let mut text_right = field.right() - 14.0;
        if !input.trim().is_empty() {
            let add = self.label_button(Id::ReminderAdd, field.right() - 5.0, field.y + 5.0, 26.0, 12.0, 0.0, "Hinzufügen", ts(11.5).bold(), WHITE, Paint::Grad, None, true);
            text_right = add.x - 8.0;
        }
        let area = Rect::new(field.x + 36.0, field.y, text_right - field.x - 36.0, field.h);
        let st = ts(12.5);
        let caret_x = if input.is_empty() {
            self.g.text("z. B. „20 min Wäsche“ oder „um 18:30 Essen“", area, st.vcenter().trim(), TEXT3);
            area.x
        } else {
            // Long text scrolls so the end stays visible.
            let tw = self.g.measure(&input, st);
            let offset = (tw + 3.0 - area.w).max(0.0);
            self.g.push_clip(area);
            self.g.text(&input, Rect::new(area.x - offset, area.y, tw + 6.0, area.h), st.vcenter(), WHITE);
            self.g.pop_clip();
            area.x - offset + tw
        };
        if focus && self.a.caret_on {
            self.g.fill_rect(Rect::new(caret_x + 1.0, field.cy() - 8.0, 1.5, 16.0), Paint::Solid(accent()));
        }

        // Live preview of what was understood.
        let y = field.bottom() + 7.0;
        let line = Rect::new(CONTENT_X + 4.0, y, CONTENT_W - 8.0, 16.0);
        let count = self.a.reminders.len();
        match self.a.input_preview() {
            Some(Ok((text, when))) => {
                self.g.text("\u{E73E}", Rect::new(line.x, y + 1.0, 14.0, 16.0), ts(10.5).icon(), accent());
                let st = ts(11.5).bold();
                let tw = self.g.measure(&text, st).ceil().min(line.w * 0.6);
                self.g.text(&text, Rect::new(line.x + 18.0, y - 1.0, tw + 2.0, 18.0), st.trim(), WHITE);
                self.g.text(&format!("·  {when}"), Rect::new(line.x + 24.0 + tw, y, line.right() - line.x - 24.0 - tw, 16.0), ts(11.0).trim(), accent());
            }
            Some(Err(problem)) => {
                self.g.text("\u{E7BA}", Rect::new(line.x, y + 1.0, 14.0, 16.0), ts(10.5).icon(), BELL);
                self.g.text(problem, Rect::new(line.x + 18.0, y, line.w - 18.0, 16.0), ts(11.0).trim(), TEXT2);
            }
            None => {
                let hint = if focus { "Enter speichert · Esc schließt" } else { "Ins Feld klicken und lostippen" };
                self.g.text(hint, line, ts(11.0).trim(), TEXT3);
                if count > 3 {
                    self.g.text(&format!("+{} weitere", count - 3), line, ts(11.0).right(), TEXT2);
                }
            }
        }

        // List of upcoming reminders.
        let list_y = y + 24.0;
        if count == 0 {
            let area = Rect::new(CONTENT_X, list_y, CONTENT_W, 92.0);
            self.g.fill_rrect(area, 16.0, Paint::Solid(argb(0x08FFFFFF)));
            self.g.stroke_rrect(area.inset(0.5), 15.5, argb(0x26FFFFFF), 1.0, true);
            self.g.text("\u{E823}", Rect::new(area.x, area.y + 18.0, area.w, 18.0), ts(16.0).icon().center(), TEXT3);
            self.g.text("Keine Erinnerungen", Rect::new(area.x, area.y + 42.0, area.w, 18.0), ts(12.0).bold().center(), TEXT2);
            self.g.text("Zahl + Einheit + Text, oder eine Uhrzeit", Rect::new(area.x, area.y + 61.0, area.w, 16.0), ts(10.5).center(), TEXT3);
            return;
        }
        let now = unix_now();
        for i in 0..count.min(3) {
            let (id, text, due) = {
                let r = &self.a.reminders[i];
                (r.id, r.text.clone(), r.due)
            };
            let row = Rect::new(CONTENT_X, list_y + i as f32 * 32.0, CONTENT_W, 28.0);
            self.reminder_row(row, id, &text, due, now);
        }
    }

    fn reminder_row(&mut self, r: Rect, id: u32, text: &str, due: u64, now: u64) {
        let pop = self.anim(Key::PopScale(REM_POP + id), 1.0);
        let saved = self.g.alpha;
        self.g.alpha = saved * self.anim(Key::PopOpacity(REM_POP + id), 1.0);
        self.g.push_scale(r.cx(), r.cy(), pop, pop);

        self.g.fill_rrect(r, 14.0, Paint::Solid(CARD));
        self.g.text("\u{EA8F}", Rect::new(r.x + 12.0, r.y + 1.0, 14.0, r.h), ts(11.0).icon().vcenter(), BELL);
        let remove = Rect::new(r.right() - 25.0, r.y + 4.0, 20.0, 20.0);
        self.icon_button(Id::ReminderRemove(id), remove, "\u{E711}", 8.5, TEXT2, Some(Paint::Solid(argb(0x14FFFFFF))));

        let when = format!("{}  ·  {}", due_text(due), format_left(due as i64 - now as i64));
        let st = ts(10.5);
        let ww = self.g.measure(&when, st).ceil();
        let when_x = remove.x - 8.0 - ww;
        self.g.text(&when, Rect::new(when_x, r.y, ww + 2.0, r.h), st.vcenter(), TEXT2);
        self.g.text(text, Rect::new(r.x + 32.0, r.y, (when_x - r.x - 40.0).max(0.0), r.h), ts(12.0).bold().vcenter().trim(), WHITE);

        self.g.pop_xf();
        self.g.alpha = saved;
    }
}

/// The settings window: same widgets as the island, on an opaque rounded panel.
pub fn draw_settings(g: &mut Gfx, a: &mut App) {
    a.hits.clear();
    g.begin();
    let full = Rect::new(0.0, 0.0, SW_W, SW_H);
    g.fill_rrect(full, 14.0, Paint::Solid(argb(0xFF141416)));
    g.stroke_rrect(full.inset(0.5), 13.5, argb(0x33FFFFFF), 1.0, false);
    let mut cx = Cx { g, a, hit: true };
    cx.settings_page();
    cx.g.alpha = 1.0;
}
