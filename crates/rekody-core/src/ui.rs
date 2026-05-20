//! Brand palette + shared style helpers for every rekody UI surface.
//!
//! Two parallel definitions per color:
//! * a [`ratatui::style::Color`] for ratatui-backed views (history TUI)
//! * an ANSI escape string for inline CLI output (spinner, banners, lists)
//!
//! Anything that paints to the terminal pulls from here so brand drift stays
//! impossible.

use ratatui::style::Color;

// ── Brand palette: ratatui colors ──────────────────────────────────────────

pub const BRAND_TEAL: Color = Color::Rgb(0x20, 0x80, 0x8D);
pub const BRAND_TEAL_LIGHT: Color = Color::Rgb(0x4F, 0xB8, 0xC5);
pub const CREAM: Color = Color::Rgb(0xFB, 0xFA, 0xF4);
pub const FG: Color = Color::Rgb(0xE8, 0xE8, 0xE8);
pub const DIM: Color = Color::Rgb(0x77, 0x77, 0x77);
pub const SUBTLE: Color = Color::Rgb(0x55, 0x55, 0x55);
pub const OK_GREEN: Color = Color::Rgb(0x6B, 0xCB, 0x77);
pub const WARN_AMBER: Color = Color::Rgb(0xE6, 0xB4, 0x50);
pub const SLOW_RED: Color = Color::Rgb(0xD9, 0x6B, 0x6B);

// ── Brand palette: ANSI escapes ────────────────────────────────────────────

pub const BRAND: &str = "\x1b[38;2;32;128;141m"; // #20808D
pub const BRAND_LIGHT: &str = "\x1b[38;2;79;184;197m"; // #4FB8C5
pub const CREAM_ANSI: &str = "\x1b[38;2;251;250;244m"; // #FBFAF4
pub const DIM_ANSI: &str = "\x1b[38;2;119;119;119m";
pub const SUBTLE_ANSI: &str = "\x1b[38;2;85;85;85m";
pub const OK_ANSI: &str = "\x1b[38;2;107;203;119m";
pub const WARN_ANSI: &str = "\x1b[38;2;230;180;80m";
pub const SLOW_ANSI: &str = "\x1b[38;2;217;107;107m";
pub const BOLD: &str = "\x1b[1m";
pub const RESET: &str = "\x1b[0m";

// ── Helpers ────────────────────────────────────────────────────────────────

/// Latency-bucket color for an end-to-end millisecond figure.
/// `<5s` green, `<15s` amber, otherwise red — matches the history TUI dots.
pub fn latency_ansi(total_ms: u64) -> &'static str {
    match total_ms {
        0..=4_999 => OK_ANSI,
        5_000..=14_999 => WARN_ANSI,
        _ => SLOW_ANSI,
    }
}

/// Mid-dot separator in dim brand gray, e.g. for `foo · bar · baz` rows.
pub fn sep() -> String {
    format!("{DIM_ANSI}·{RESET}")
}

/// Top of a card: `  ╭─  title  ·  subtitle` (subtitle optional).
/// Closes with [`card_bottom`] for a paired open/close pair.
pub fn card_top(title: &str, subtitle: Option<&str>) -> String {
    let head = format!("  {BRAND}╭─{RESET}  {BRAND_LIGHT}{BOLD}{title}{RESET}");
    match subtitle {
        Some(s) if !s.is_empty() => format!("{head}  {sep}  {DIM_ANSI}{s}{RESET}", sep = sep()),
        _ => head,
    }
}

/// A bare brand-teal vertical rail: `  │`.
pub fn card_rail() -> String {
    format!("  {BRAND}│{RESET}")
}

/// Bottom of a card: `  ╰─…─` rule, with an optional inline trailing note.
pub fn card_bottom(width: usize, note: Option<&str>) -> String {
    let rule = "─".repeat(width);
    match note {
        Some(n) if !n.is_empty() => {
            format!("  {BRAND}╰{rule}{RESET}  {DIM_ANSI}{n}{RESET}", rule = rule)
        }
        _ => format!("  {BRAND}╰{rule}{RESET}", rule = rule),
    }
}
