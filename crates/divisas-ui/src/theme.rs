//! Design tokens for Divisas Desktop.
//!
//! Dark-first fintech theme: calm neutrals, one accent color, red/green
//! reserved strictly for money direction. Airy spacing — generous padding,
//! larger gaps, content capped at a readable width.
#![allow(dead_code)]

use gpui::{Background, Hsla, Rgba, linear_color_stop, linear_gradient};

// ----------------------------------------------------------------------
// Color palette — DeFi dark: deep purple-black, violet accent
// ----------------------------------------------------------------------

pub struct Palette;

impl Palette {
    // Backgrounds (layered, purple-tinted dark)
    pub const ROOT: u32 = 0x0D0B14;
    pub const SURFACE: u32 = 0x16121F;
    pub const SURFACE_HOVER: u32 = 0x1F1A2C;
    pub const SURFACE_SUNKEN: u32 = 0x0F0D17;
    pub const SIDEBAR: u32 = 0x12101A;

    // Borders
    pub const BORDER: u32 = 0x251F33;
    pub const BORDER_STRONG: u32 = 0x352C4A;

    // Text
    pub const TEXT: u32 = 0xF2EFF9;
    pub const TEXT_SECONDARY: u32 = 0xA79FC0;
    pub const TEXT_TERTIARY: u32 = 0x6B6280;

    // Accent — violet, the only "interactive" color
    pub const ACCENT: u32 = 0x8B7CFF;
    pub const ACCENT_HOVER: u32 = 0x9E92FF;
    pub const ACCENT_SOFT: u32 = 0x262043; // tinted surface for selected states
    pub const ACCENT_TEXT: u32 = 0xB4ABFF;

    // Semantic — money direction only
    pub const UP: u32 = 0x34D399;
    pub const UP_SOFT: u32 = 0x12291F;
    pub const DOWN: u32 = 0xF87171;
    pub const DOWN_SOFT: u32 = 0x2D1518;
    pub const WARN: u32 = 0xF59E0B;

    // Secondary accent — amber highlight card
    pub const AMBER: u32 = 0xF59E0B;
    pub const AMBER_SOFT: u32 = 0x2D2113;
    pub const AMBER_TEXT: u32 = 0xFBBF24;

    // Misc
    pub const WHITE: u32 = 0xFFFFFF;
}

/// Const-friendly hex → Rgba (gpui's `rgb()` is not const).
pub const fn hex(code: u32) -> Rgba {
    let [_, r, g, b] = code.to_be_bytes();
    Rgba {
        r: r as f32 / 255.0,
        g: g as f32 / 255.0,
        b: b as f32 / 255.0,
        a: 1.0,
    }
}

/// Semantic aliases for reuse in components.
pub const BG_ROOT: Rgba = hex(Palette::ROOT);
pub const BG_SURFACE: Rgba = hex(Palette::SURFACE);
pub const BG_SURFACE_HOVER: Rgba = hex(Palette::SURFACE_HOVER);
pub const BG_SIDEBAR: Rgba = hex(Palette::SIDEBAR);
pub const BORDER: Rgba = hex(Palette::BORDER);
pub const TEXT_PRIMARY: Rgba = hex(Palette::TEXT);
pub const TEXT_SECONDARY: Rgba = hex(Palette::TEXT_SECONDARY);
pub const TEXT_TERTIARY: Rgba = hex(Palette::TEXT_TERTIARY);
pub const ACCENT: Rgba = hex(Palette::ACCENT);
pub const ACCENT_HOVER: Rgba = hex(Palette::ACCENT_HOVER);
pub const ACCENT_SOFT: Rgba = hex(Palette::ACCENT_SOFT);
pub const ACCENT_TEXT: Rgba = hex(Palette::ACCENT_TEXT);
pub const UP: Rgba = hex(Palette::UP);
pub const UP_SOFT: Rgba = hex(Palette::UP_SOFT);
pub const DOWN: Rgba = hex(Palette::DOWN);
pub const DOWN_SOFT: Rgba = hex(Palette::DOWN_SOFT);
pub const WARN: Rgba = hex(Palette::WARN);
pub const AMBER: Rgba = hex(Palette::AMBER);
pub const AMBER_SOFT: Rgba = hex(Palette::AMBER_SOFT);
pub const AMBER_TEXT: Rgba = hex(Palette::AMBER_TEXT);
pub const WHITE: Rgba = hex(Palette::WHITE);

// ----------------------------------------------------------------------
// Spacing scale (px)
// ----------------------------------------------------------------------
// Airier than default: 8-pt rhythm with generous section padding.

pub const SPACE_XS: f32 = 4.;
pub const SPACE_SM: f32 = 8.;
pub const SPACE_MD: f32 = 12.;
pub const SPACE_LG: f32 = 16.;
pub const SPACE_XL: f32 = 24.;
pub const SPACE_2XL: f32 = 32.;
pub const SPACE_3XL: f32 = 48.;

// ----------------------------------------------------------------------
// Typography
// ----------------------------------------------------------------------

pub const TEXT_XS: f32 = 11.;
pub const TEXT_SM: f32 = 12.;
pub const TEXT_BASE: f32 = 13.;
pub const TEXT_MD: f32 = 15.;
pub const TEXT_LG: f32 = 18.;
pub const TEXT_XL: f32 = 22.;
pub const TEXT_DISPLAY: f32 = 28.; // hero numbers

pub const RADIUS_SM: f32 = 6.;
pub const RADIUS_MD: f32 = 10.;
pub const RADIUS_LG: f32 = 14.;
pub const RADIUS_XL: f32 = 18.;

/// Purple gradient for hero/stat cards (135°, accent → deep tint).
pub fn accent_card_gradient() -> Background {
    linear_gradient(
        135.,
        linear_color_stop(hex(Palette::ACCENT), 0.42),
        linear_color_stop(hex(Palette::ACCENT_SOFT), 1.),
    )
}

/// Softer violet wash for secondary gradient cards.
pub fn accent_soft_gradient() -> Background {
    linear_gradient(
        135.,
        linear_color_stop(hex(Palette::ACCENT), 0.14),
        linear_color_stop(hex(Palette::SURFACE), 1.),
    )
}

/// Chart fill gradient: accent fading to transparent, top → bottom.
pub fn chart_fill_gradient() -> Background {
    let Rgba { r, g, b, .. } = hex(Palette::ACCENT);
    let top = Rgba { r, g, b, a: 0.38 };
    let bottom = Rgba { r, g, b, a: 0.0 };
    linear_gradient(180., linear_color_stop(top, 0.), linear_color_stop(bottom, 1.))
}

/// Tabular-figure-friendly formatting for rates.
pub fn format_rate(value: f64) -> String {
    format!("{value:.2}")
}

/// Money formatting with thousands separators, e.g. "$12,450.00".
pub fn format_usd(value: f64) -> String {
    format_money(value)
}

pub fn format_money(value: f64) -> String {
    // No thousands separators in Rust format strings; insert manually.
    let formatted = format!("{value:.2}");
    let (integer, decimals) = formatted.split_once('.').unwrap_or((&formatted, "00"));
    let mut grouped = String::new();
    for (index, byte) in integer.bytes().enumerate() {
        if index > 0 && (integer.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(byte as char);
    }
    format!("${grouped}.{decimals}")
}

/// Parse an ISO timestamp into a short "14:32" time or "Mar 12" date.
pub fn short_time(iso: &str) -> String {
    // ISO 8601: take "2026-01-01T14:32:55..." → "14:32"
    let time = iso.split('T').nth(1).unwrap_or("");
    if time.len() >= 5 {
        return time[..5].to_string();
    }
    iso.to_string()
}

pub fn short_date(iso: &str) -> String {
    let date = iso.split('T').next().unwrap_or(iso);
    date.to_string()
}

/// Change direction from the backend's "increase"/"decrease"/"equal".
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Direction {
    Up,
    Down,
    Flat,
    Unknown,
}

pub fn parse_direction(value: Option<&str>) -> Direction {
    match value {
        Some("increase") => Direction::Up,
        Some("decrease") => Direction::Down,
        Some("equal") => Direction::Flat,
        _ => Direction::Unknown,
    }
}

impl Direction {
    pub fn arrow(self) -> &'static str {
        match self {
            Direction::Up => "▲",
            Direction::Down => "▼",
            Direction::Flat => "—",
            Direction::Unknown => "·",
        }
    }

    pub fn color(self) -> Rgba {
        match self {
            Direction::Up => UP,
            Direction::Down => DOWN,
            Direction::Flat => TEXT_SECONDARY,
            Direction::Unknown => TEXT_TERTIARY,
        }
    }
}

/// Minutes since an ISO-8601 UTC timestamp, if parseable. Used for staleness.
pub fn minutes_since(iso: &str) -> Option<u64> {
    // Minimal ISO-8601 (UTC) parse: YYYY-MM-DDTHH:MM:SS(.sss)(Z|+00:00)
    let bytes = iso.as_bytes();
    if bytes.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    let year: i64 = iso.get(0..4)?.parse().ok()?;
    let month: i64 = iso.get(5..7)?.parse().ok()?;
    let day: i64 = iso.get(8..10)?.parse().ok()?;
    let hour: i64 = iso.get(11..13)?.parse().ok()?;
    let minute: i64 = iso.get(14..16)?.parse().ok()?;
    let second: i64 = iso.get(17..19)?.parse().ok()?;

    let days = days_from_civil(year, month, day);
    let timestamp = days * 86_400 + hour * 3600 + minute * 60 + second;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    Some((now - timestamp).max(0) as u64)
}

/// Howard Hinnant's days_from_civil algorithm.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`]: epoch days → (year, month, day).
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// ISO date (YYYY-MM-DD) for N days ago, UTC — for history range queries.
pub fn iso_date_days_ago(days: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (y, m, d) = civil_from_days((now / 86_400) as i64 - days as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Human-readable "time ago" string.
pub fn time_ago(minutes: u64) -> String {
    match minutes {
        0..=1 => "just now".into(),
        2..=59 => format!("{minutes} min ago"),
        60..=1439 => format!("{} h ago", minutes / 60),
        1440..=43199 => format!("{} d ago", minutes / 1440),
        _ => format!("{} mo ago", minutes / 43200),
    }
}

/// True if data is older than 30 minutes.
pub fn is_stale(minutes: u64) -> bool {
    minutes > 30
}

#[allow(dead_code)]
fn unused_hsla(_: Hsla) {
    let _ = Hsla {
        h: 0.,
        s: 0.,
        l: 0.,
        a: 1.,
    };
}
