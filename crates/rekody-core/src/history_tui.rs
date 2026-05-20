//! Interactive history TUI for `rekody history -i`.
//!
//! A ratatui-based browser for transcription history with live search,
//! per-entry detail, and one-keystroke clipboard copy. Designed to feel
//! native to the rekody brand — teal accents, soft spacing, no clutter.
//!
//! Keybindings:
//!   ↑/k, ↓/j         navigate
//!   g, G             jump to top/bottom
//!   Ctrl+u, Ctrl+d   page up/down
//!   /                start search (Esc to clear)
//!   Enter            copy selected entry to clipboard
//!   f                toggle raw vs cleaned text
//!   ?                show help
//!   q, Esc, Ctrl+C   quit

use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState, Padding, Paragraph, Wrap};

use crate::history::{History, HistoryEntry};

// ── Brand palette ───────────────────────────────────────────────────────────

const BRAND_TEAL: Color = Color::Rgb(0x20, 0x80, 0x8D);
const BRAND_TEAL_LIGHT: Color = Color::Rgb(0x4F, 0xB8, 0xC5);
const DIM: Color = Color::Rgb(0x77, 0x77, 0x77);
const SUBTLE: Color = Color::Rgb(0x55, 0x55, 0x55);
const FG: Color = Color::Rgb(0xE8, 0xE8, 0xE8);
const FG_BOLD: Color = Color::Rgb(0xFB, 0xFA, 0xF4); // brand cream
const OK: Color = Color::Rgb(0x6B, 0xCB, 0x77); // green latency
const WARN: Color = Color::Rgb(0xE6, 0xB4, 0x50); // amber latency
const SLOW: Color = Color::Rgb(0xD9, 0x6B, 0x6B); // red latency

// ── Public entrypoint ───────────────────────────────────────────────────────

pub fn run(history: &History) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let entries: Vec<&HistoryEntry> = history.entries().iter().rev().collect();
    let mut app = App::new(entries);
    let result = run_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

// ── App state ───────────────────────────────────────────────────────────────

struct App<'a> {
    /// Reversed view (newest first) into the on-disk history.
    entries: Vec<&'a HistoryEntry>,
    /// Indices into `entries`, narrowed by the active search query.
    filtered: Vec<usize>,
    /// Position within `filtered`.
    list_state: ListState,
    search: String,
    search_active: bool,
    full_view: bool,
    show_help: bool,
    flash: Option<Flash>,
    quit: bool,
}

struct Flash {
    msg: String,
    until: Instant,
    ok: bool,
}

impl<'a> App<'a> {
    fn new(entries: Vec<&'a HistoryEntry>) -> Self {
        let filtered: Vec<usize> = (0..entries.len()).collect();
        let mut list_state = ListState::default();
        if !filtered.is_empty() {
            list_state.select(Some(0));
        }
        Self {
            entries,
            filtered,
            list_state,
            search: String::new(),
            search_active: false,
            full_view: false,
            show_help: false,
            flash: None,
            quit: false,
        }
    }

    fn refilter(&mut self) {
        let q = self.search.to_lowercase();
        self.filtered = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                if q.is_empty() {
                    return true;
                }
                e.text.to_lowercase().contains(&q)
                    || e.raw_transcript.to_lowercase().contains(&q)
                    || e.app_context.to_lowercase().contains(&q)
            })
            .map(|(i, _)| i)
            .collect();
        if self.filtered.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(0));
        }
    }

    fn selected(&self) -> Option<&HistoryEntry> {
        let pos = self.list_state.selected()?;
        let idx = *self.filtered.get(pos)?;
        self.entries.get(idx).copied()
    }

    fn move_down(&mut self, n: usize) {
        if self.filtered.is_empty() {
            return;
        }
        let max = self.filtered.len() - 1;
        let cur = self.list_state.selected().unwrap_or(0);
        self.list_state.select(Some((cur + n).min(max)));
    }

    fn move_up(&mut self, n: usize) {
        if self.filtered.is_empty() {
            return;
        }
        let cur = self.list_state.selected().unwrap_or(0);
        self.list_state.select(Some(cur.saturating_sub(n)));
    }

    fn jump_top(&mut self) {
        if !self.filtered.is_empty() {
            self.list_state.select(Some(0));
        }
    }

    fn jump_bottom(&mut self) {
        if !self.filtered.is_empty() {
            self.list_state.select(Some(self.filtered.len() - 1));
        }
    }

    fn copy_selected(&mut self) {
        let Some(entry) = self.selected() else {
            self.set_flash("nothing selected", false);
            return;
        };
        let text = entry.text.clone();
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(text)) {
            Ok(_) => self.set_flash("copied to clipboard", true),
            Err(e) => self.set_flash(format!("copy failed: {}", e), false),
        }
    }

    fn set_flash(&mut self, msg: impl Into<String>, ok: bool) {
        self.flash = Some(Flash {
            msg: msg.into(),
            until: Instant::now() + Duration::from_millis(1400),
            ok,
        });
    }

    fn flash_active(&self) -> Option<&Flash> {
        self.flash.as_ref().filter(|f| Instant::now() < f.until)
    }
}

// ── Event loop ──────────────────────────────────────────────────────────────

fn run_loop<B: ratatui::backend::Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    while !app.quit {
        terminal.draw(|f| draw(f, app))?;

        // Poll briefly so the flash banner can expire on its own.
        if !event::poll(Duration::from_millis(150))? {
            continue;
        }
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            handle_key(app, key);
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) {
    // Help overlay: any key dismisses it.
    if app.show_help {
        app.show_help = false;
        return;
    }

    if app.search_active {
        match key.code {
            KeyCode::Esc => {
                app.search.clear();
                app.search_active = false;
                app.refilter();
            }
            KeyCode::Enter => {
                app.search_active = false;
            }
            KeyCode::Backspace => {
                app.search.pop();
                app.refilter();
            }
            KeyCode::Char(c) => {
                app.search.push(c);
                app.refilter();
            }
            _ => {}
        }
        return;
    }

    // Ctrl+C always quits.
    if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
        app.quit = true;
        return;
    }

    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => app.quit = true,
        KeyCode::Char('j') | KeyCode::Down => app.move_down(1),
        KeyCode::Char('k') | KeyCode::Up => app.move_up(1),
        KeyCode::Char('g') => app.jump_top(),
        KeyCode::Char('G') => app.jump_bottom(),
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => app.move_down(10),
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => app.move_up(10),
        KeyCode::PageDown => app.move_down(10),
        KeyCode::PageUp => app.move_up(10),
        KeyCode::Char('/') => {
            app.search_active = true;
        }
        KeyCode::Char('f') => app.full_view = !app.full_view,
        KeyCode::Char('?') => app.show_help = true,
        KeyCode::Enter => app.copy_selected(),
        _ => {}
    }
}

// ── Rendering ───────────────────────────────────────────────────────────────

fn draw(f: &mut ratatui::Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // top title bar
            Constraint::Length(if app.search_active || !app.search.is_empty() {
                1
            } else {
                0
            }),
            Constraint::Min(8),                                 // list
            Constraint::Length(detail_height(f.area().height)), // detail
            Constraint::Length(1),                              // status / flash
        ])
        .split(f.area());

    draw_title(f, chunks[0], app);
    if app.search_active || !app.search.is_empty() {
        draw_search(f, chunks[1], app);
    }
    draw_list(f, chunks[2], app);
    draw_detail(f, chunks[3], app);
    draw_status(f, chunks[4], app);

    if app.show_help {
        draw_help_overlay(f);
    }
}

fn detail_height(total: u16) -> u16 {
    // Detail pane gets ~30% but never less than 7 rows or more than 14.
    ((total as f32 * 0.30) as u16).clamp(7, 14)
}

fn draw_title(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let total = app.entries.len();
    let shown = app.filtered.len();
    let count_text = if shown == total {
        format!("{} entries", total)
    } else {
        format!("{} of {} entries", shown, total)
    };

    let title = Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "rekody history",
            Style::default()
                .fg(BRAND_TEAL_LIGHT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("·", Style::default().fg(SUBTLE)),
        Span::raw("  "),
        Span::styled(count_text, Style::default().fg(DIM)),
    ]);
    f.render_widget(Paragraph::new(title), area);
}

fn draw_search(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let cursor = if app.search_active { "▏" } else { " " };
    let line = Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "/",
            Style::default()
                .fg(BRAND_TEAL_LIGHT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(&app.search, Style::default().fg(FG_BOLD)),
        Span::styled(cursor, Style::default().fg(BRAND_TEAL_LIGHT)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn draw_list(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(Style::default().fg(SUBTLE))
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);

    if app.filtered.is_empty() {
        f.render_widget(block, area);
        let msg = if app.search.is_empty() {
            "no dictation history yet — start dictating!"
        } else {
            "no entries match — try a different query"
        };
        let p = Paragraph::new(Line::from(vec![Span::styled(
            msg,
            Style::default().fg(DIM).add_modifier(Modifier::ITALIC),
        )]))
        .alignment(Alignment::Center);
        let inner2 = Rect {
            y: inner.y + inner.height / 2,
            ..inner
        };
        f.render_widget(p, inner2);
        return;
    }

    let mut state = app.list_state.clone();
    let items: Vec<ListItem> = app
        .filtered
        .iter()
        .map(|&idx| render_row(app.entries[idx], app.entries.len() - idx, &app.search))
        .collect();

    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .bg(Color::Rgb(0x14, 0x2A, 0x2D))
                .fg(FG_BOLD)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▸ ");
    f.render_stateful_widget(list, area, &mut state);
}

fn render_row<'a>(entry: &'a HistoryEntry, n: usize, search: &str) -> ListItem<'a> {
    let time = entry.timestamp.get(11..16).unwrap_or("--:--").to_string();
    let lang = detect_language(&entry.text);
    let total_ms = entry.stt_latency_ms + entry.llm_latency_ms.unwrap_or(0);
    let dot_color = latency_color(total_ms);

    let preview = preview_text(&entry.text, 64);

    let mut spans = vec![
        Span::styled(format!("  {time}  "), Style::default().fg(DIM)),
        Span::styled("●", Style::default().fg(dot_color)),
        Span::raw("  "),
        Span::styled(format!("{:<3}", lang), Style::default().fg(SUBTLE)),
        Span::raw("  "),
    ];
    spans.extend(highlight_text(&preview, search));
    spans.push(Span::raw("  "));
    spans.push(Span::styled(format!("#{n}"), Style::default().fg(SUBTLE)));

    ListItem::new(Line::from(spans))
}

fn highlight_text(text: &str, query: &str) -> Vec<Span<'static>> {
    if query.is_empty() {
        return vec![Span::styled(text.to_string(), Style::default().fg(FG))];
    }
    let lower_text = text.to_lowercase();
    let lower_q = query.to_lowercase();
    let mut spans: Vec<Span> = Vec::new();
    let mut cursor = 0usize;
    while let Some(pos) = lower_text[cursor..].find(&lower_q) {
        let abs = cursor + pos;
        if abs > cursor {
            spans.push(Span::styled(
                text[cursor..abs].to_string(),
                Style::default().fg(FG),
            ));
        }
        let end = abs + query.len();
        spans.push(Span::styled(
            text[abs..end].to_string(),
            Style::default()
                .fg(BRAND_TEAL_LIGHT)
                .add_modifier(Modifier::BOLD),
        ));
        cursor = end;
    }
    if cursor < text.len() {
        spans.push(Span::styled(
            text[cursor..].to_string(),
            Style::default().fg(FG),
        ));
    }
    spans
}

fn draw_detail(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_style(Style::default().fg(SUBTLE))
        .title(Line::from(vec![Span::styled(
            " detail ",
            Style::default().fg(DIM),
        )]))
        .padding(Padding::horizontal(2));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let Some(entry) = app.selected() else { return };
    let date = entry.timestamp.get(..10).unwrap_or("");
    let time = entry.timestamp.get(11..19).unwrap_or("--:--:--");
    let lang = detect_language(&entry.text);
    let lat = match entry.llm_latency_ms {
        Some(llm) => format!("{}ms STT · {}ms LLM", entry.stt_latency_ms, llm),
        None => format!("{}ms STT", entry.stt_latency_ms),
    };
    let app_label = if entry.app_context.eq_ignore_ascii_case("unknown") {
        "—".to_string()
    } else {
        entry.app_context.clone()
    };
    let text = if app.full_view {
        &entry.raw_transcript
    } else {
        &entry.text
    };
    let view_label = if app.full_view { "raw" } else { "cleaned" };

    let header = Line::from(vec![
        Span::styled(
            format!("{date} {time}"),
            Style::default().fg(BRAND_TEAL_LIGHT),
        ),
        Span::styled("  ·  ", Style::default().fg(SUBTLE)),
        Span::styled(lang, Style::default().fg(DIM)),
        Span::styled("  ·  ", Style::default().fg(SUBTLE)),
        Span::styled(lat, Style::default().fg(DIM)),
        Span::styled("  ·  ", Style::default().fg(SUBTLE)),
        Span::styled(app_label, Style::default().fg(DIM)),
        Span::styled("  ·  ", Style::default().fg(SUBTLE)),
        Span::styled(
            view_label,
            Style::default()
                .fg(BRAND_TEAL)
                .add_modifier(Modifier::ITALIC),
        ),
    ]);

    let body = Paragraph::new(vec![
        header,
        Line::from(""),
        Line::from(Span::styled(text.clone(), Style::default().fg(FG_BOLD))),
    ])
    .wrap(Wrap { trim: false });
    f.render_widget(body, inner);
}

fn draw_status(f: &mut ratatui::Frame, area: Rect, app: &App) {
    if let Some(flash) = app.flash_active() {
        let color = if flash.ok { OK } else { SLOW };
        let glyph = if flash.ok { "✓" } else { "✗" };
        let line = Line::from(vec![
            Span::raw("  "),
            Span::styled(
                glyph,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(flash.msg.clone(), Style::default().fg(FG_BOLD)),
        ]);
        f.render_widget(Paragraph::new(line), area);
        return;
    }

    let hints = [
        ("↑↓", "nav"),
        ("/", "search"),
        ("⏎", "copy"),
        ("f", "raw"),
        ("?", "help"),
        ("q", "quit"),
    ];
    let mut spans = vec![Span::raw("  ")];
    for (i, (k, v)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ·  ", Style::default().fg(SUBTLE)));
        }
        spans.push(Span::styled(
            (*k).to_string(),
            Style::default()
                .fg(BRAND_TEAL_LIGHT)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled((*v).to_string(), Style::default().fg(DIM)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_help_overlay(f: &mut ratatui::Frame) {
    let area = centered_rect(60, 60, f.area());
    let lines = vec![
        Line::from(Span::styled(
            "  rekody history — keybindings",
            Style::default()
                .fg(BRAND_TEAL_LIGHT)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        kb_line("↑ k", "move up"),
        kb_line("↓ j", "move down"),
        kb_line("g / G", "jump top / bottom"),
        kb_line("Ctrl-u / Ctrl-d", "page up / down"),
        kb_line("/", "start search"),
        kb_line("Esc", "clear search · close help · quit"),
        kb_line("Enter", "copy selected to clipboard"),
        kb_line("f", "toggle raw vs cleaned text"),
        kb_line("?", "show this help"),
        kb_line("q · Ctrl-c", "quit"),
        Line::from(""),
        Line::from(Span::styled(
            "  press any key to dismiss",
            Style::default().fg(DIM).add_modifier(Modifier::ITALIC),
        )),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(BRAND_TEAL))
        .padding(Padding::new(1, 1, 1, 1))
        .title(Line::from(Span::styled(
            " help ",
            Style::default()
                .fg(BRAND_TEAL_LIGHT)
                .add_modifier(Modifier::BOLD),
        )));
    let p = Paragraph::new(lines).block(block);
    f.render_widget(ratatui::widgets::Clear, area);
    f.render_widget(p, area);
}

fn kb_line(key: &str, desc: &str) -> Line<'static> {
    Line::from(vec![
        Span::raw("  "),
        Span::styled(
            format!("{:<18}", key),
            Style::default().fg(BRAND_TEAL_LIGHT),
        ),
        Span::styled(desc.to_string(), Style::default().fg(FG)),
    ])
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup[1])[1]
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn latency_color(total_ms: u64) -> Color {
    match total_ms {
        0..=4_999 => OK,
        5_000..=14_999 => WARN,
        _ => SLOW,
    }
}

/// Lightweight heuristic: classify text as English ("en") or likely-Kalenjin
/// ("kln"). Far from perfect — meant only as a visual hint in the list.
fn detect_language(text: &str) -> &'static str {
    let lower = text.to_lowercase();
    if lower.is_empty() {
        return "—";
    }
    // English markers: very common closed-class tokens that almost never
    // surface in Kalenjin text.
    const EN_MARKERS: &[&str] = &[
        " the ", " and ", " is ", " are ", " was ", " were ", " of ", " for ", " that ", " this ",
        " with ", " you ", " they ", " what ", " from ",
    ];
    let padded = format!(" {} ", lower);
    if EN_MARKERS.iter().any(|m| padded.contains(m)) {
        return "en";
    }
    // Kalenjin markers: orthographic patterns that almost never appear in English.
    if lower.contains("ng'") || lower.contains("ng’") {
        return "kln";
    }
    // Default: leave ambiguous. Don't pretend to know.
    "—"
}

fn preview_text(text: &str, max: usize) -> String {
    let collapsed: String = text
        .chars()
        .map(|c| if c == '\n' { ' ' } else { c })
        .collect();
    let trimmed = collapsed.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let mut out: String = trimmed.chars().take(max - 1).collect();
    out.push('…');
    out
}
