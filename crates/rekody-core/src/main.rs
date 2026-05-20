//! rekody — voice dictation CLI
//!
//! Entrypoint for the rekody binary. Handles subcommand dispatch and the
//! polished inline TUI for the live dictation pipeline.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;
use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use rekody_core::onboarding;
use rekody_core::ui::{
    BOLD, BRAND, BRAND_LIGHT, CREAM_ANSI as CREAM, DIM_ANSI as DIM, OK_ANSI as OK, RESET,
    SLOW_ANSI as SLOW, SUBTLE_ANSI as SUBTLE, WARN_ANSI as WARN, card_bottom, card_rail, card_top,
    latency_ansi, sep,
};
use rekody_core::{Pipeline, RekodyConfig, load_config};

// ── CLI definition ─────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "rekody",
    version = env!("CARGO_PKG_VERSION"),
    about = "Voice dictation — speak, it types",
    disable_help_subcommand = true,
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,

    /// Enable verbose tracing output
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Bypass VAD — capture every frame (use for media-playback transcription)
    #[arg(long)]
    record_all_audio: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run first-time setup or reconfigure
    Setup,
    /// Show or edit current configuration
    Config {
        #[command(subcommand)]
        action: Option<ConfigCmd>,
    },
    /// Browse dictation history
    History {
        /// Number of entries to display (default: 20)
        #[arg(short, long, default_value = "20")]
        count: usize,
        /// Filter by text content (case-insensitive)
        #[arg(short, long)]
        search: Option<String>,
        /// Filter by app name (e.g. "VS Code", "Terminal")
        #[arg(short, long)]
        app: Option<String>,
        /// Show full transcript text (not truncated)
        #[arg(short, long)]
        full: bool,
        /// Show session statistics summary
        #[arg(long)]
        stats: bool,
        /// Output raw JSON (pipe-friendly)
        #[arg(long)]
        json: bool,
        /// Copy the Nth most-recent entry to the clipboard (1 = latest)
        #[arg(long, value_name = "N")]
        copy: Option<usize>,
        /// Open the interactive history browser (search, copy, navigate)
        #[arg(short, long)]
        interactive: bool,
    },
    /// Check STT and LLM provider connectivity
    Doctor,
    /// Manage API keys stored in the system keychain
    Key {
        #[command(subcommand)]
        action: KeyCmd,
    },
    /// Check for and install the latest version
    Update {
        /// Only check — don't install
        #[arg(long)]
        check: bool,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Print current configuration (default)
    Show,
    /// Open config file in $EDITOR
    Edit,
    /// Print the path of the config file
    Path,
}

#[derive(Subcommand)]
enum KeyCmd {
    /// Save an API key for a provider (prompts securely)
    Set {
        /// Provider: groq, deepgram, anthropic, openai, gemini, cerebras
        provider: String,
    },
    /// Remove a stored API key
    Delete {
        /// Provider name
        provider: String,
    },
    /// List which providers have keys stored
    List,
}

// ── ASCII banner ─────────────────────────────────────────────────────────────

fn print_ascii_banner() {
    // "rekody" rendered in figlet `nancyj`, gradient teal→blue.
    const ART: &[&str] = &[
        r#"                  dP                      dP          "#,
        r#"                  88                      88          "#,
        r#"88d888b. .d8888b. 88  .dP  .d8888b. .d888b88 dP    dP "#,
        r#"88'  `88 88ooood8 88888"   88'  `88 88'  `88 88    88 "#,
        r#"88       88.  ... 88  `8b. 88.  .88 88.  .88 88.  .88 "#,
        r#"dP       `88888P' dP   `YP `88888P' `88888P8 `8888P88 "#,
        r#"                                                  .88 "#,
        r#"                                              d8888P  "#,
    ];
    // Gradient stays inside the rekody teal family:
    //   top:    #4FB8C5  (lightened brand teal — luminous on dark terminals)
    //   bottom: #20808D  (brand teal, exact)
    // Ends on-brand; anchors the eye on the canonical color.
    const TOP: (u8, u8, u8) = (0x4F, 0xB8, 0xC5);
    const BOT: (u8, u8, u8) = (0x20, 0x80, 0x8D);
    let n = ART.len();
    for (i, line) in ART.iter().enumerate() {
        let t = i as f32 / (n - 1) as f32;
        let r = (TOP.0 as f32 + (BOT.0 as f32 - TOP.0 as f32) * t) as u8;
        let g = (TOP.1 as f32 + (BOT.1 as f32 - TOP.1 as f32) * t) as u8;
        let b = (TOP.2 as f32 + (BOT.2 as f32 - TOP.2 as f32) * t) as u8;
        eprintln!("\x1b[38;2;{r};{g};{b}m{line}\x1b[0m");
    }
    // Tagline in brand teal, exact.
    eprintln!(
        "\x1b[38;2;{};{};{}mvoice dictation for everyone\x1b[0m\n",
        BOT.0, BOT.1, BOT.2
    );
}

// ── Entry point ─────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None => run_dictation(cli.verbose, cli.record_all_audio).await,
        Some(Cmd::Setup) => cmd_setup(),
        Some(Cmd::Config { action }) => cmd_config(action),
        Some(Cmd::History {
            count,
            search,
            app,
            full,
            stats,
            json,
            copy,
            interactive,
        }) => cmd_history(count, search, app, full, stats, json, copy, interactive),
        Some(Cmd::Doctor) => cmd_doctor().await,
        Some(Cmd::Key { action }) => cmd_key(action),
        Some(Cmd::Update { check }) => cmd_update(check).await,
    }
}

// ── Subcommand: setup ────────────────────────────────────────────────────────

fn cmd_setup() -> Result<()> {
    print_ascii_banner();
    onboarding::run_onboarding()
}

// ── Subcommand: config ───────────────────────────────────────────────────────

fn cmd_config(action: Option<ConfigCmd>) -> Result<()> {
    let config_path = find_config_path();
    let config = load_config_or_default(&config_path);

    match action.unwrap_or(ConfigCmd::Show) {
        ConfigCmd::Show => print_config(&config, &config_path),
        ConfigCmd::Path => match &config_path {
            Some(p) => println!("{}", p),
            None => println!("  {WARN}{BOLD}!{RESET}  {CREAM}no config file found{RESET}"),
        },
        ConfigCmd::Edit => {
            let path = match &config_path {
                Some(p) => p.clone(),
                None => {
                    let default = default_config_path();
                    println!(
                        "  {BRAND_LIGHT}{BOLD}+{RESET}  {CREAM}creating config{RESET}  {sep}  {DIM}{}{RESET}",
                        default,
                        sep = sep()
                    );
                    default
                }
            };
            let editor = std::env::var("EDITOR")
                .or_else(|_| std::env::var("VISUAL"))
                .unwrap_or_else(|_| "nano".to_string());
            std::process::Command::new(&editor).arg(&path).status()?;
        }
    }
    Ok(())
}

fn print_config(config: &RekodyConfig, path: &Option<String>) {
    let subtitle = match path {
        Some(p) => p.clone(),
        None => "no config file — using defaults".to_string(),
    };
    let rail = card_rail();

    println!();
    println!("{}", card_top("rekody config", Some(&subtitle)));
    println!("{rail}");

    // STT section
    let stt_display = stt_display_name(config);
    println!("{rail}   {BRAND_LIGHT}{BOLD}STT{RESET}");
    println!(
        "{rail}     {DIM}engine {RESET}  {CREAM}{BOLD}{}{RESET}",
        stt_display
    );
    if let Some(key) = &config.deepgram_api_key {
        println!(
            "{rail}     {DIM}key    {RESET}  {DIM}{}{RESET}",
            mask_key(key)
        );
    }
    println!("{rail}");

    // LLM Providers section
    println!("{rail}   {BRAND_LIGHT}{BOLD}LLM providers{RESET}");
    if config.providers.is_empty() {
        let has_groq = config.groq_api_key.as_ref().is_some_and(|k| !k.is_empty());
        let has_cerebras = config
            .cerebras_api_key
            .as_ref()
            .is_some_and(|k| !k.is_empty());
        if has_groq {
            println!(
                "{rail}     {DIM}1{RESET}  {CREAM}{BOLD}groq{RESET}  {DIM}(legacy key){RESET}"
            );
        }
        if has_cerebras {
            println!(
                "{rail}     {DIM}2{RESET}  {CREAM}{BOLD}cerebras{RESET}  {DIM}(legacy key){RESET}"
            );
        }
        if !has_groq && !has_cerebras {
            println!(
                "{rail}     {WARN}{BOLD}!{RESET}  {CREAM}none configured{RESET}  {sep}  {DIM}run: rekody setup{RESET}",
                sep = sep()
            );
        }
    } else {
        for (i, p) in config.providers.iter().enumerate() {
            let key_hint = if p.api_key.is_empty() {
                format!("{WARN}(no key){RESET}")
            } else {
                format!("{DIM}{}{RESET}", mask_key(&p.api_key))
            };
            println!(
                "{rail}     {DIM}{}{RESET}  {CREAM}{BOLD}{}/{}{RESET}  {}",
                i + 1,
                p.name,
                p.model,
                key_hint,
            );
        }
    }
    println!("{rail}");

    // Options section
    println!("{rail}   {BRAND_LIGHT}{BOLD}options{RESET}");
    println!(
        "{rail}     {DIM}mode   {RESET}  {CREAM}{BOLD}{}{RESET}",
        format_activation_mode(&config.activation_mode)
    );
    println!(
        "{rail}     {DIM}inject {RESET}  {CREAM}{BOLD}{}{RESET}",
        config.injection_method
    );
    println!(
        "{rail}     {DIM}vad    {RESET}  {CREAM}{BOLD}{}{RESET}",
        config.vad_threshold
    );
    let vad_mode = if config.record_all_audio {
        "off — every frame captured (--record-all-audio)"
    } else {
        "on — RMS gating"
    };
    println!("{rail}     {DIM}gate   {RESET}  {CREAM}{}{RESET}", vad_mode);
    println!("{rail}");
    println!("{}", card_bottom(48, None));
    println!();
}

/// Mask an API key, showing only the last 4 characters.
fn mask_key(key: &str) -> String {
    if key.len() <= 4 {
        return "████".to_string();
    }
    let visible = &key[key.len() - 4..];
    format!("████████████{}", visible)
}

// ── Subcommand: history ──────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn cmd_history(
    count: usize,
    search: Option<String>,
    app_filter: Option<String>,
    full: bool,
    stats: bool,
    json_out: bool,
    copy_nth: Option<usize>,
    interactive: bool,
) -> Result<()> {
    let history = rekody_core::history::History::load();
    let all = history.entries();

    if interactive {
        return rekody_core::history_tui::run(&history);
    }

    // --copy N: copy the Nth most-recent entry to clipboard and exit.
    if let Some(n) = copy_nth {
        let n = n.max(1);
        let entry = all.iter().rev().nth(n - 1);
        println!();
        match entry {
            Some(e) => {
                let mut clipboard = arboard::Clipboard::new()?;
                clipboard.set_text(&e.text)?;
                let preview = if e.text.len() > 70 {
                    format!("{}…", &e.text[..69])
                } else {
                    e.text.clone()
                };
                println!(
                    "  {OK}{BOLD}✓{RESET}  {CREAM}copied entry #{n}{RESET}  {sep}  {DIM}clipboard{RESET}",
                    sep = sep()
                );
                println!("     {DIM}{}{RESET}", preview);
            }
            None => {
                println!(
                    "  {SLOW}{BOLD}✗{RESET}  {CREAM}no entry #{n}{RESET}  {sep}  {DIM}{} total{RESET}",
                    all.len(),
                    sep = sep()
                );
            }
        }
        println!();
        return Ok(());
    }

    // Apply filters
    let filtered: Vec<_> = all
        .iter()
        .filter(|e| {
            if let Some(ref q) = search {
                let q = q.to_lowercase();
                if !e.text.to_lowercase().contains(&q)
                    && !e.raw_transcript.to_lowercase().contains(&q)
                {
                    return false;
                }
            }
            if let Some(ref app) = app_filter
                && !e.app_context.to_lowercase().contains(&app.to_lowercase())
            {
                return false;
            }
            true
        })
        .collect();

    let shown: Vec<_> = filtered.iter().rev().take(count).collect();

    // JSON output (pipe-friendly)
    if json_out {
        println!("{}", serde_json::to_string_pretty(&shown)?);
        return Ok(());
    }

    // Compose card subtitle: entry count + active filter, if any.
    let filter_note = if let Some(ref q) = search {
        Some(format!("search \"{}\"", q))
    } else {
        app_filter.as_ref().map(|a| format!("app \"{}\"", a))
    };
    let subtitle = match (filter_note.as_deref(), filtered.len(), all.len()) {
        (Some(f), m, t) if m != t => format!("{} of {} entries  {sep}  {}", m, t, f, sep = sep()),
        (Some(f), _, t) => format!("{} entries  {sep}  {}", t, f, sep = sep()),
        (None, _, t) => format!("{} entries", t),
    };

    let head = if stats { "history · stats" } else { "history" };

    println!();
    println!("{}", card_top(head, Some(&subtitle)));
    println!("{}", card_rail());

    // Stats view
    if stats || shown.is_empty() {
        let total = all.len();
        let avg_stt = if total > 0 {
            all.iter().map(|e| e.stt_latency_ms).sum::<u64>() / total as u64
        } else {
            0
        };
        let avg_llm = {
            let llm_entries: Vec<_> = all.iter().filter_map(|e| e.llm_latency_ms).collect();
            if llm_entries.is_empty() {
                None
            } else {
                Some(llm_entries.iter().sum::<u64>() / llm_entries.len() as u64)
            }
        };

        // App breakdown
        let mut app_counts: std::collections::HashMap<&str, usize> =
            std::collections::HashMap::new();
        for e in all {
            *app_counts.entry(e.app_context.as_str()).or_insert(0) += 1;
        }
        let mut app_sorted: Vec<_> = app_counts.iter().collect();
        app_sorted.sort_by(|a, b| b.1.cmp(a.1));

        let avg_total = avg_stt + avg_llm.unwrap_or(0);
        let dot = latency_ansi(avg_total);
        println!(
            "{rail}   {DIM}total       {RESET}  {CREAM}{BOLD}{}{RESET}  {sep}  {DIM}all-time{RESET}",
            total,
            rail = card_rail(),
            sep = sep()
        );
        let lat = match avg_llm {
            Some(l) => format!("{}ms STT  {sep}  {}ms LLM", avg_stt, l, sep = sep()),
            None => format!("{}ms STT", avg_stt),
        };
        println!(
            "{rail}   {DIM}avg latency {RESET}  {dot}●{RESET}  {CREAM}{}{RESET}",
            lat,
            rail = card_rail(),
        );
        if !app_sorted.is_empty() {
            println!("{}", card_rail());
            println!("{rail}   {DIM}top apps    {RESET}", rail = card_rail());
            for (app, count) in app_sorted.iter().take(5) {
                let app_disp = if app.len() > 28 {
                    format!("{}…", &app[..27])
                } else {
                    app.to_string()
                };
                let noun = if **count == 1 {
                    "dictation"
                } else {
                    "dictations"
                };
                println!(
                    "{rail}      {CREAM}{:<28}{RESET}  {DIM}{} {}{RESET}",
                    app_disp,
                    count,
                    noun,
                    rail = card_rail(),
                );
            }
        }

        if shown.is_empty() {
            println!("{}", card_rail());
            let msg = if search.is_some() || app_filter.is_some() {
                format!("{WARN}no entries match the filter{RESET}")
            } else {
                format!("{DIM}no history yet — start dictating!{RESET}")
            };
            println!("{rail}   {}", msg, rail = card_rail());
            println!("{}", card_bottom(48, None));
            println!();
            return Ok(());
        }
        if stats {
            // Stats-only view: stats already printed, no entry list follows.
            println!("{}", card_bottom(48, None));
            println!();
            return Ok(());
        }
        // Empty `shown` was handled above; fall through to listing for the
        // mixed (stats + entries) case.
    }

    // Entry listing grouped by date.
    let mut last_date = String::new();
    for entry in &shown {
        let date = entry.timestamp.get(..10).unwrap_or("").to_string();
        if date != last_date {
            if !last_date.is_empty() {
                println!("{}", card_rail());
            }
            println!(
                "{rail}   {SUBTLE}{BOLD}{}{RESET}",
                &date,
                rail = card_rail()
            );
            last_date = date;
        }

        let time = entry.timestamp.get(11..16).unwrap_or("--:--");
        let total_ms = entry.stt_latency_ms + entry.llm_latency_ms.unwrap_or(0);
        let dot = latency_ansi(total_ms);
        let latency = match entry.llm_latency_ms {
            Some(llm) => format!(
                "{}ms STT {sep} {}ms LLM",
                entry.stt_latency_ms,
                llm,
                sep = sep()
            ),
            None => format!("{}ms STT", entry.stt_latency_ms),
        };
        let app_col = if entry.app_context.len() > 18 {
            format!("{}…", &entry.app_context[..17])
        } else {
            entry.app_context.clone()
        };

        if full {
            println!(
                "{rail}   {DIM}{}{RESET}  {dot}●{RESET}  {DIM}{:<18}  {}{RESET}",
                time,
                app_col,
                latency,
                rail = card_rail(),
            );
            println!(
                "{rail}      {CREAM}{}{RESET}",
                &entry.text,
                rail = card_rail()
            );
            if entry.raw_transcript != entry.text {
                println!(
                    "{rail}      {DIM}raw:  {}{RESET}",
                    &entry.raw_transcript,
                    rail = card_rail()
                );
            }
        } else {
            let max_text = 56;
            let text = if entry.text.len() > max_text {
                format!("{}…", &entry.text[..max_text - 1])
            } else {
                entry.text.clone()
            };
            println!(
                "{rail}   {DIM}{}{RESET}  {dot}●{RESET}  {CREAM}{:<56}{RESET}  {DIM}{:<18}{RESET}  {DIM}{}{RESET}",
                time,
                text,
                app_col,
                latency,
                rail = card_rail(),
            );
        }
    }

    let note = if shown.len() < filtered.len() {
        format!(
            "showing {} of {}  {sep}  -c {} for more  {sep}  -i to browse",
            shown.len(),
            filtered.len(),
            filtered.len(),
            sep = sep()
        )
    } else {
        format!("showing {}  {sep}  -i to browse", shown.len(), sep = sep())
    };
    println!("{}", card_rail());
    println!("{}", card_bottom(48, Some(&note)));
    println!();
    Ok(())
}

// ── Subcommand: doctor ───────────────────────────────────────────────────────

async fn cmd_doctor() -> Result<()> {
    let config_path = find_config_path();
    let config = load_config_or_default(&config_path);
    let rail = card_rail();

    println!();
    println!(
        "{}",
        card_top("rekody doctor", Some("provider health check"))
    );
    println!("{rail}");

    // STT check
    println!("{rail}   {BRAND_LIGHT}{BOLD}STT{RESET}");
    let stt_name = stt_display_name(&config);
    match config.stt_engine.to_lowercase().as_str() {
        "deepgram" => {
            let key = config.deepgram_api_key.as_deref().unwrap_or("");
            if key.is_empty() {
                println!(
                    "{rail}     {SLOW}{BOLD}✗{RESET}  {CREAM}{}{RESET}  {sep}  {WARN}no API key{RESET}  {DIM}— rekody key set deepgram{RESET}",
                    stt_name,
                    sep = sep()
                );
            } else {
                let t = Instant::now();
                let ok = reqwest::Client::new()
                    .get("https://api.deepgram.com/v1/projects")
                    .header("Authorization", format!("Token {}", key))
                    .send()
                    .await
                    .map(|r| r.status().is_success())
                    .unwrap_or(false);
                let ms = t.elapsed().as_millis();
                if ok {
                    let dot = latency_ansi(ms as u64);
                    println!(
                        "{rail}     {OK}{BOLD}✓{RESET}  {CREAM}{}{RESET}  {sep}  {dot}●{RESET} {DIM}{}ms{RESET}",
                        stt_name,
                        ms,
                        sep = sep()
                    );
                } else {
                    println!(
                        "{rail}     {SLOW}{BOLD}✗{RESET}  {CREAM}{}{RESET}  {sep}  {WARN}auth failed{RESET}  {DIM}— rekody key set deepgram{RESET}",
                        stt_name,
                        sep = sep()
                    );
                }
            }
        }
        "groq" => {
            let key = config.groq_api_key.as_deref().unwrap_or("");
            check_openai_compat_provider(
                "Groq Whisper",
                "https://api.groq.com/openai/v1/models",
                key,
            )
            .await;
        }
        _ => {
            println!(
                "{rail}     {BRAND_LIGHT}○{RESET}  {CREAM}local whisper{RESET}  {sep}  {DIM}no network check needed{RESET}",
                sep = sep()
            );
        }
    }
    println!("{rail}");

    // LLM providers
    println!("{rail}   {BRAND_LIGHT}{BOLD}LLM{RESET}");
    if config.providers.is_empty()
        && config.groq_api_key.is_none()
        && config.cerebras_api_key.is_none()
    {
        println!(
            "{rail}     {WARN}{BOLD}!{RESET}  {CREAM}none configured{RESET}  {sep}  {DIM}run: rekody setup{RESET}",
            sep = sep()
        );
    } else if !config.providers.is_empty() {
        for p in &config.providers {
            match p.name.as_str() {
                "ollama" | "lm-studio" | "vllm" => {
                    let url = p.base_url.as_deref().unwrap_or("http://localhost:11434");
                    let t = Instant::now();
                    let ok = reqwest::Client::new().get(url).send().await.is_ok();
                    let ms = t.elapsed().as_millis();
                    if ok {
                        let dot = latency_ansi(ms as u64);
                        println!(
                            "{rail}     {OK}{BOLD}✓{RESET}  {CREAM}{}{RESET}{DIM}/{}{RESET}  {sep}  {dot}●{RESET} {DIM}{}ms{RESET}",
                            p.name,
                            p.model,
                            ms,
                            sep = sep()
                        );
                    } else {
                        println!(
                            "{rail}     {SLOW}{BOLD}✗{RESET}  {CREAM}{}{RESET}{DIM}/{}{RESET}  {sep}  {WARN}not running{RESET}",
                            p.name,
                            p.model,
                            sep = sep()
                        );
                    }
                }
                "gemini" => {
                    let url = "https://generativelanguage.googleapis.com/v1beta/openai/models";
                    check_openai_compat_provider_keyed(
                        &format!("{}/{}", p.name, p.model),
                        url,
                        &p.api_key,
                        "x-goog-api-key",
                    )
                    .await;
                }
                _ => {
                    let url = p
                        .base_url
                        .clone()
                        .unwrap_or_else(|| provider_models_url(&p.name));
                    check_openai_compat_provider(
                        &format!("{}/{}", p.name, p.model),
                        &url,
                        &p.api_key,
                    )
                    .await;
                }
            }
        }
    } else {
        // Legacy
        if let Some(key) = &config.groq_api_key {
            check_openai_compat_provider("groq", "https://api.groq.com/openai/v1/models", key)
                .await;
        }
        if let Some(key) = &config.cerebras_api_key {
            check_openai_compat_provider("cerebras", "https://api.cerebras.ai/v1/models", key)
                .await;
        }
    }
    println!("{rail}");

    // System
    println!("{rail}   {BRAND_LIGHT}{BOLD}system{RESET}");
    #[cfg(target_os = "macos")]
    {
        let mic = check_macos_permission("kTCCServiceMicrophone");
        let acc = check_macos_permission("kTCCServiceAccessibility");
        print_permission("microphone", mic);
        print_permission("accessibility", acc);
    }
    #[cfg(not(target_os = "macos"))]
    {
        println!(
            "{rail}     {BRAND_LIGHT}○{RESET}  {DIM}system checks not available on this platform{RESET}"
        );
    }
    println!("{rail}");
    println!("{}", card_bottom(48, None));
    println!();

    Ok(())
}

async fn check_openai_compat_provider(label: &str, url: &str, key: &str) {
    let rail = card_rail();
    if key.is_empty() {
        println!(
            "{rail}     {SLOW}{BOLD}✗{RESET}  {CREAM}{}{RESET}  {sep}  {WARN}no API key{RESET}  {DIM}— rekody key set <provider>{RESET}",
            label,
            sep = sep()
        );
        return;
    }
    let t = Instant::now();
    let ok = reqwest::Client::new()
        .get(url)
        .bearer_auth(key)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);
    let ms = t.elapsed().as_millis();
    if ok {
        let dot = latency_ansi(ms as u64);
        println!(
            "{rail}     {OK}{BOLD}✓{RESET}  {CREAM}{}{RESET}  {sep}  {dot}●{RESET} {DIM}{}ms{RESET}",
            label,
            ms,
            sep = sep()
        );
    } else {
        println!(
            "{rail}     {SLOW}{BOLD}✗{RESET}  {CREAM}{}{RESET}  {sep}  {WARN}auth failed{RESET}  {DIM}— check your API key{RESET}",
            label,
            sep = sep()
        );
    }
}

async fn check_openai_compat_provider_keyed(label: &str, url: &str, key: &str, header: &str) {
    let rail = card_rail();
    if key.is_empty() {
        println!(
            "{rail}     {SLOW}{BOLD}✗{RESET}  {CREAM}{}{RESET}  {sep}  {WARN}no API key{RESET}",
            label,
            sep = sep()
        );
        return;
    }
    let t = Instant::now();
    let ok = reqwest::Client::new()
        .get(url)
        .header(header, key)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);
    let ms = t.elapsed().as_millis();
    if ok {
        let dot = latency_ansi(ms as u64);
        println!(
            "{rail}     {OK}{BOLD}✓{RESET}  {CREAM}{}{RESET}  {sep}  {dot}●{RESET} {DIM}{}ms{RESET}",
            label,
            ms,
            sep = sep()
        );
    } else {
        println!(
            "{rail}     {SLOW}{BOLD}✗{RESET}  {CREAM}{}{RESET}  {sep}  {WARN}auth failed{RESET}  {DIM}— check your API key{RESET}",
            label,
            sep = sep()
        );
    }
}

fn provider_models_url(name: &str) -> String {
    match name {
        "groq" => "https://api.groq.com/openai/v1/models",
        "cerebras" => "https://api.cerebras.ai/v1/models",
        "openai" => "https://api.openai.com/v1/models",
        "together" => "https://api.together.xyz/v1/models",
        "openrouter" => "https://openrouter.ai/api/v1/models",
        "fireworks" => "https://api.fireworks.ai/inference/v1/models",
        "anthropic" => "https://api.anthropic.com/v1/models",
        _ => "http://localhost:11434/v1/models",
    }
    .to_string()
}

#[cfg(target_os = "macos")]
fn check_macos_permission(service: &str) -> MicCheck {
    if service == "kTCCServiceAccessibility" {
        return if rekody_hotkey::is_accessibility_trusted() {
            MicCheck::Granted
        } else {
            MicCheck::Denied
        };
    }
    // Microphone: probe the default input device via cpal. This fires the
    // macOS TCC prompt on first access and returns Denied synchronously if
    // the user has already blocked access. See rekody_audio::probe_microphone.
    match rekody_audio::probe_microphone() {
        rekody_audio::MicStatus::Granted => MicCheck::Granted,
        rekody_audio::MicStatus::Denied => MicCheck::Denied,
        rekody_audio::MicStatus::NoDevice => MicCheck::NoDevice,
        rekody_audio::MicStatus::Unknown => MicCheck::Unknown,
    }
}

/// Tri-state result for macOS permission checks in `doctor`.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MicCheck {
    Granted,
    Denied,
    NoDevice,
    Unknown,
}

#[cfg(target_os = "macos")]
fn print_permission(name: &str, status: MicCheck) {
    let rail = card_rail();
    match status {
        MicCheck::Granted => {
            println!("{rail}     {OK}{BOLD}✓{RESET}  {CREAM}{}{RESET}", name);
        }
        MicCheck::Denied => {
            println!(
                "{rail}     {SLOW}{BOLD}✗{RESET}  {CREAM}{}{RESET}  {sep}  {WARN}open System Settings → Privacy{RESET}",
                name,
                sep = sep()
            );
        }
        MicCheck::NoDevice => {
            println!(
                "{rail}     {WARN}{BOLD}…{RESET}  {CREAM}{}{RESET}  {sep}  {DIM}no input device detected{RESET}",
                name,
                sep = sep()
            );
        }
        MicCheck::Unknown => {
            println!(
                "{rail}     {WARN}{BOLD}…{RESET}  {CREAM}{}{RESET}  {sep}  {DIM}could not probe — try recording{RESET}",
                name,
                sep = sep()
            );
        }
    }
}

// ── Subcommand: update ───────────────────────────────────────────────────────

fn is_homebrew_install(path: &std::path::Path) -> bool {
    let s = path.to_string_lossy();
    s.contains("/Cellar/") || s.contains("/homebrew/")
}

async fn cmd_update(check_only: bool) -> Result<()> {
    const CURRENT: &str = env!("CARGO_PKG_VERSION");
    const REPO: &str = "rekody/rekody";

    let subtitle = if check_only {
        "release check"
    } else {
        "self-update"
    };
    println!();
    println!("{}", card_top("rekody update", Some(subtitle)));
    println!("{}", card_rail());
    println!(
        "{rail}   {DIM}current     {RESET}  {CREAM}{BOLD}v{CURRENT}{RESET}",
        rail = card_rail()
    );

    let client = reqwest::Client::builder()
        .user_agent("rekody-updater")
        .build()?;

    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = client.get(&url).send().await?;

    if !resp.status().is_success() {
        println!(
            "{rail}   {SLOW}{BOLD}✗{RESET}  {CREAM}could not reach GitHub{RESET}  {sep}  {DIM}check your connection{RESET}",
            rail = card_rail(),
            sep = sep()
        );
        println!("{}", card_bottom(48, None));
        println!();
        return Ok(());
    }

    let release: serde_json::Value = resp.json().await?;
    let latest_tag = release["tag_name"]
        .as_str()
        .unwrap_or("")
        .trim_start_matches('v');

    println!(
        "{rail}   {DIM}latest      {RESET}  {CREAM}{BOLD}v{latest_tag}{RESET}",
        rail = card_rail()
    );

    // Simple semver comparison (major.minor.patch)
    fn parse_ver(s: &str) -> (u64, u64, u64) {
        let mut parts = s.splitn(3, '.').map(|p| p.parse::<u64>().unwrap_or(0));
        (
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
        )
    }

    if latest_tag == CURRENT || parse_ver(latest_tag) <= parse_ver(CURRENT) {
        println!("{}", card_rail());
        println!(
            "{rail}   {OK}{BOLD}✓{RESET}  {CREAM}already on the latest version{RESET}",
            rail = card_rail()
        );
        println!("{}", card_bottom(48, None));
        println!();
        return Ok(());
    }

    println!(
        "{rail}   {DIM}status      {RESET}  {WARN}{BOLD}update available{RESET}  {sep}  {DIM}v{} → v{}{RESET}",
        CURRENT,
        latest_tag,
        rail = card_rail(),
        sep = sep()
    );

    // Resolve where the running binary actually lives, following symlinks.
    let install_path = match std::env::current_exe().and_then(|p| p.canonicalize()) {
        Ok(p) => p,
        Err(e) => {
            println!("{}", card_rail());
            println!(
                "{rail}   {SLOW}{BOLD}✗{RESET}  {CREAM}could not resolve current binary{RESET}  {sep}  {DIM}{}{RESET}",
                e,
                rail = card_rail(),
                sep = sep()
            );
            println!("{}", card_bottom(48, None));
            println!();
            return Ok(());
        }
    };

    let owned_by_brew = is_homebrew_install(&install_path);

    if check_only {
        let cmd = if owned_by_brew {
            "brew upgrade rekody"
        } else {
            "rekody update"
        };
        println!("{}", card_rail());
        println!(
            "{}",
            card_bottom(48, Some(&format!("run `{}` to install", cmd)))
        );
        println!();
        return Ok(());
    }

    // Defer to package manager when one owns this install.
    if owned_by_brew {
        println!("{}", card_rail());
        println!(
            "{rail}   {BRAND_LIGHT}{BOLD}ℹ{RESET}  {CREAM}managed by Homebrew{RESET}",
            rail = card_rail()
        );
        println!(
            "{}",
            card_bottom(48, Some("run `brew upgrade rekody` to upgrade"))
        );
        println!();
        return Ok(());
    }

    println!("{}", card_rail());

    // Detect platform/arch
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let platform = match os {
        "macos" => "macos",
        "linux" => "linux",
        other => {
            println!(
                "{rail}   {SLOW}{BOLD}✗{RESET}  {CREAM}unsupported OS{RESET}  {sep}  {DIM}{}{RESET}",
                other,
                rail = card_rail(),
                sep = sep()
            );
            println!(
                "{}",
                card_bottom(
                    48,
                    Some(&format!("download from github.com/{REPO}/releases"))
                )
            );
            println!();
            return Ok(());
        }
    };
    let arch_name = match arch {
        "aarch64" => "aarch64",
        "x86_64" => "x86_64",
        other => {
            println!(
                "{rail}   {SLOW}{BOLD}✗{RESET}  {CREAM}unsupported arch{RESET}  {sep}  {DIM}{}{RESET}",
                other,
                rail = card_rail(),
                sep = sep()
            );
            println!("{}", card_bottom(48, None));
            println!();
            return Ok(());
        }
    };

    let tarball = format!("rekody-{latest_tag}-{platform}-{arch_name}.tar.gz");
    let download_url =
        format!("https://github.com/{REPO}/releases/download/v{latest_tag}/{tarball}");

    println!(
        "{rail}   {BRAND_LIGHT}{BOLD}↓{RESET}  {CREAM}downloading{RESET}  {sep}  {DIM}{}{RESET}",
        tarball,
        rail = card_rail(),
        sep = sep()
    );

    let resp = client.get(&download_url).send().await?;
    if !resp.status().is_success() {
        println!(
            "{rail}   {SLOW}{BOLD}✗{RESET}  {CREAM}download failed{RESET}  {sep}  {DIM}HTTP {}{RESET}",
            resp.status(),
            rail = card_rail(),
            sep = sep()
        );
        println!(
            "{}",
            card_bottom(
                48,
                Some(&format!(
                    "fallback: curl -fsSL https://raw.githubusercontent.com/{REPO}/main/install.sh | bash"
                ))
            )
        );
        println!();
        return Ok(());
    }
    let bytes = resp.bytes().await?;
    // gzip magic bytes: 1f 8b
    if bytes.len() < 2 || bytes[0] != 0x1f || bytes[1] != 0x8b {
        println!(
            "{rail}   {SLOW}{BOLD}✗{RESET}  {CREAM}invalid gzip archive{RESET}",
            rail = card_rail()
        );
        println!(
            "{}",
            card_bottom(
                48,
                Some(&format!(
                    "fallback: curl -fsSL https://raw.githubusercontent.com/{REPO}/main/install.sh | bash"
                ))
            )
        );
        println!();
        return Ok(());
    }

    // Unpack into a temp dir
    let tmp = std::env::temp_dir().join(format!("rekody-update-{latest_tag}"));
    std::fs::create_dir_all(&tmp)?;
    let tarball_path = tmp.join(&tarball);
    std::fs::write(&tarball_path, &bytes)?;

    let status = std::process::Command::new("tar")
        .args([
            "-xzf",
            tarball_path.to_str().unwrap(),
            "-C",
            tmp.to_str().unwrap(),
        ])
        .status()?;

    if !status.success() {
        println!(
            "{rail}   {SLOW}{BOLD}✗{RESET}  {CREAM}failed to extract tarball{RESET}",
            rail = card_rail()
        );
        println!("{}", card_bottom(48, None));
        println!();
        return Ok(());
    }

    let new_bin = tmp.join("rekody");

    // Atomic replace via rename (works while the running binary is in-use on POSIX).
    let staged = install_path.with_file_name(format!(
        ".rekody.update-{}.{}",
        latest_tag,
        std::process::id()
    ));

    let install_ok = std::fs::copy(&new_bin, &staged)
        .and_then(|_| std::fs::rename(&staged, &install_path))
        .is_ok();

    if !install_ok {
        let _ = std::fs::remove_file(&staged);
        let sudo = std::process::Command::new("sudo")
            .args([
                "install",
                "-m",
                "0755",
                new_bin.to_str().unwrap(),
                install_path.to_str().unwrap(),
            ])
            .status()?;
        if !sudo.success() {
            println!(
                "{rail}   {SLOW}{BOLD}✗{RESET}  {CREAM}could not write to {}{RESET}",
                install_path.display(),
                rail = card_rail()
            );
            println!("{}", card_bottom(48, Some("try running with sudo")));
            println!();
            return Ok(());
        }
    } else {
        let _ = std::process::Command::new("chmod")
            .args(["+x", install_path.to_str().unwrap()])
            .status();
    }

    let _ = std::fs::remove_dir_all(&tmp);

    println!(
        "{rail}   {OK}{BOLD}✓{RESET}  {CREAM}updated to {BRAND_LIGHT}v{}{CREAM}{RESET}  {sep}  {DIM}was v{}{RESET}",
        latest_tag,
        CURRENT,
        rail = card_rail(),
        sep = sep()
    );
    println!(
        "{}",
        card_bottom(48, Some("restart any running rekody process"))
    );
    println!();
    Ok(())
}

// ── Subcommand: key ──────────────────────────────────────────────────────────

fn cmd_key(action: KeyCmd) -> Result<()> {
    match action {
        KeyCmd::Set { provider } => {
            use std::io::{self, Write};
            println!();
            print!(
                "  {BRAND_LIGHT}{BOLD}rekody key{RESET}  {sep}  {DIM}enter API key for{RESET}  {CREAM}{BOLD}{}{RESET}  {DIM}(hidden):{RESET} ",
                provider,
                sep = sep()
            );
            io::stdout().flush()?;
            let key = rpassword_read_password(&provider)?;
            if key.trim().is_empty() {
                println!("\n  {WARN}{BOLD}!{RESET}  {CREAM}no key entered — aborted{RESET}\n");
                return Ok(());
            }
            save_keychain_key(&provider, key.trim())?;
            println!(
                "\n  {OK}{BOLD}✓{RESET}  {CREAM}{} key saved{RESET}  {sep}  {DIM}macOS keychain{RESET}\n",
                provider,
                sep = sep()
            );
        }
        KeyCmd::Delete { provider } => {
            println!();
            match delete_keychain_key(&provider) {
                Ok(_) => println!(
                    "  {OK}{BOLD}✓{RESET}  {CREAM}{} key deleted{RESET}\n",
                    provider
                ),
                Err(_) => println!(
                    "  {DIM}○  no key found for{RESET}  {CREAM}{}{RESET}\n",
                    provider
                ),
            }
        }
        KeyCmd::List => {
            let rail = card_rail();
            println!();
            println!("{}", card_top("rekody keys", Some("keychain status")));
            println!("{rail}");
            let providers = &[
                "groq",
                "deepgram",
                "anthropic",
                "openai",
                "gemini",
                "cerebras",
                "together",
                "openrouter",
                "fireworks",
            ];
            let mut any = false;
            for p in providers {
                match get_keychain_key(p) {
                    Ok(key) if !key.is_empty() => {
                        println!(
                            "{rail}   {OK}{BOLD}✓{RESET}  {CREAM}{BOLD}{:<11}{RESET}  {sep}  {DIM}{}{RESET}",
                            p,
                            mask_key(&key),
                            sep = sep()
                        );
                        any = true;
                    }
                    _ => {
                        println!(
                            "{rail}   {DIM}○  {:<11}{RESET}  {sep}  {DIM}—{RESET}",
                            p,
                            sep = sep()
                        );
                    }
                }
            }
            println!("{rail}");
            let footer = if any {
                None
            } else {
                Some("no keys stored — rekody key set <provider>")
            };
            println!("{}", card_bottom(48, footer));
            println!();
        }
    }
    Ok(())
}

fn rpassword_read_password(_provider: &str) -> Result<String> {
    // Simple stdin read (terminal should handle echo=off via stty if needed)
    // Use rpassword-style approach: disable echo
    #[cfg(unix)]
    {
        // Disable echo via termios
        let fd = std::os::unix::io::AsRawFd::as_raw_fd(&std::io::stdin());
        let mut term: libc::termios = unsafe { std::mem::zeroed() };
        unsafe { libc::tcgetattr(fd, &mut term) };
        let mut noecho = term;
        noecho.c_lflag &= !libc::ECHO;
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &noecho) };

        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf)?;

        unsafe { libc::tcsetattr(fd, libc::TCSANOW, &term) };
        Ok(buf.trim_end_matches('\n').to_string())
    }
    #[cfg(not(unix))]
    {
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf)?;
        Ok(buf.trim_end_matches('\n').to_string())
    }
}

fn save_keychain_key(provider: &str, key: &str) -> Result<()> {
    let entry = keyring::Entry::new("com.rekody.voice", provider)?;
    entry.set_password(key)?;
    Ok(())
}

fn delete_keychain_key(provider: &str) -> Result<()> {
    let entry = keyring::Entry::new("com.rekody.voice", provider)?;
    entry.delete_credential()?;
    Ok(())
}

fn get_keychain_key(provider: &str) -> Result<String> {
    let entry = keyring::Entry::new("com.rekody.voice", provider)?;
    Ok(entry.get_password()?)
}

// ── Config helpers ───────────────────────────────────────────────────────────

fn find_config_path() -> Option<String> {
    let candidates = [
        dirs::home_dir().map(|h| h.join(".config").join("rekody").join("config.toml")),
        dirs::config_dir().map(|c| c.join("rekody").join("config.toml")),
        Some(std::path::PathBuf::from("config/default.toml")),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|p| p.exists())
        .map(|p| p.to_string_lossy().to_string())
}

fn default_config_path() -> String {
    dirs::home_dir()
        .map(|h| {
            h.join(".config")
                .join("rekody")
                .join("config.toml")
                .to_string_lossy()
                .to_string()
        })
        .unwrap_or_else(|| "~/.config/rekody/config.toml".to_string())
}

fn load_config_or_default(path: &Option<String>) -> RekodyConfig {
    path.as_deref()
        .and_then(|p| load_config(p).ok())
        .unwrap_or_default()
}

fn stt_display_name(config: &RekodyConfig) -> String {
    match config.stt_engine.to_lowercase().as_str() {
        "groq" => "Groq Cloud Whisper Large v3".to_string(),
        "deepgram" => "Deepgram Nova-3".to_string(),
        "cohere" => format!("Cohere local (port {})", config.cohere_stt_port),
        _ => format!("Local Whisper ({})", config.whisper_model),
    }
}

fn format_activation_mode(mode: &str) -> &str {
    match mode.to_lowercase().as_str() {
        "toggle" => "toggle — tap ⌥Space to start/stop",
        _ => "push-to-talk — hold ⌥Space",
    }
}

// ── Live dictation pipeline ──────────────────────────────────────────────────

async fn run_dictation(verbose: bool, record_all_audio_flag: bool) -> Result<()> {
    // If no config exists, run onboarding first.
    if onboarding::needs_onboarding() {
        onboarding::run_onboarding()?;
    }

    let config_path = find_config_path();
    let mut config = load_config_or_default(&config_path);

    // Pull missing API keys from the keychain into config at runtime.
    inject_keychain_keys(&mut config);

    // CLI flag wins over config: --record-all-audio always forces it on.
    if record_all_audio_flag {
        config.record_all_audio = true;
    }

    // Print the startup banner.
    print_banner(&config);

    // Create the status spinner.
    let spinner = ProgressBar::new_spinner();
    set_idle_style(&spinner);

    // Session stats tracker.
    let session = Arc::new(SessionStats::new());

    // Set up tracing with our custom UI layer.
    let ui_layer = UiLayer::new(spinner.clone(), Arc::clone(&session));

    let level = if verbose { "debug" } else { "info" };
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| format!("{},rekody=debug", level).parse().unwrap());

    // DEBUG: tee tracing events to file when REKODY_DEBUG_LOG=<path> is set.
    let debug_layer = std::env::var("REKODY_DEBUG_LOG")
        .ok()
        .and_then(|p| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .ok()
        })
        .map(|f| {
            tracing_subscriber::fmt::layer()
                .with_writer(std::sync::Mutex::new(f))
                .with_target(true)
                .with_thread_ids(true)
                .with_ansi(false)
        });

    tracing_subscriber::registry()
        .with(env_filter)
        .with(ui_layer)
        .with(debug_layer)
        .init();

    let pipeline = Pipeline::new(config)?;
    pipeline.run().await?;

    spinner.finish_and_clear();
    Ok(())
}

/// Pull API keys from the keychain into the config struct if they are missing.
fn inject_keychain_keys(config: &mut RekodyConfig) {
    // Deepgram STT key
    if config.deepgram_api_key.is_none() || config.deepgram_api_key.as_deref() == Some("") {
        if let Ok(key) = get_keychain_key("deepgram")
            && !key.is_empty()
        {
            config.deepgram_api_key = Some(key);
        }
        // Also try the legacy account name
        if config.deepgram_api_key.is_none()
            && let Ok(key) = get_keychain_key("deepgram_api_key")
            && !key.is_empty()
        {
            config.deepgram_api_key = Some(key);
        }
    }
    // Groq key for STT or LLM
    if (config.groq_api_key.is_none() || config.groq_api_key.as_deref() == Some(""))
        && let Ok(key) = get_keychain_key("groq")
        && !key.is_empty()
    {
        config.groq_api_key = Some(key.clone());
        // Update existing groq provider entry, or create one if absent.
        let existing = config.providers.iter_mut().find(|p| p.name == "groq");
        if let Some(p) = existing {
            if p.api_key.is_empty() {
                p.api_key = key;
            }
        } else {
            config.providers.push(rekody_core::ProviderConfig {
                name: "groq".into(),
                api_key: key,
                model: "openai/gpt-oss-20b".into(),
                base_url: None,
            });
        }
    }
    // Inject keychain keys into any providers array entries that lack a key.
    for p in config.providers.iter_mut() {
        if p.api_key.is_empty()
            && let Ok(key) = get_keychain_key(&p.name)
            && !key.is_empty()
        {
            p.api_key = key;
        }
    }
}

// ── Startup banner ───────────────────────────────────────────────────────────

fn print_banner(config: &RekodyConfig) {
    // Card-style banner: brand-teal left rail + integrated title, closed by a
    // bottom rule. The right edge is open so we never have to do ANSI-aware
    // width math. The card anchors the info block; the inline status line
    // below it stays unbordered to read as a live element, not a panel row.
    let rail = card_rail();
    let stt = stt_display_name(config);

    let llm_active = rekody_core::has_llm_providers(config);
    let llm_line = if llm_active {
        let names: Vec<_> = config
            .providers
            .iter()
            .map(|p| format!("{}/{}", p.name, p.model))
            .collect();
        format!(
            "{CREAM}{}{RESET}",
            names.join(&format!("  {BRAND}›{RESET}  "))
        )
    } else if config.providers.is_empty() {
        format!("{DIM}none{RESET}")
    } else if config.llm_enabled == Some(false) {
        format!("{DIM}off{RESET}")
    } else {
        format!("{DIM}none{RESET}  {SUBTLE}(Deepgram smart_format handles formatting){RESET}")
    };

    let mode_short = match config.activation_mode.to_lowercase().as_str() {
        "toggle" => "toggle",
        _ => "push-to-talk",
    };

    println!();
    println!(
        "{}",
        card_top("rekody", Some(&format!("v{}", env!("CARGO_PKG_VERSION"))))
    );
    println!("{rail}");
    println!("{rail}   {DIM}STT  {RESET}  {CREAM}{BOLD}{}{RESET}", stt);
    println!("{rail}   {DIM}LLM  {RESET}  {}", llm_line);
    println!("{rail}   {DIM}mode {RESET}  {CREAM}{}{RESET}", mode_short);
    println!("{rail}");
    println!("{}", card_bottom(48, None));
    println!();
}

// ── Session statistics ───────────────────────────────────────────────────────

struct SessionStats {
    dictation_count: AtomicU64,
    total_audio_secs: Mutex<f64>,
}

impl SessionStats {
    fn new() -> Self {
        Self {
            dictation_count: AtomicU64::new(0),
            total_audio_secs: Mutex::new(0.0),
        }
    }

    fn record(&self, audio_secs: f64) {
        self.dictation_count.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut secs) = self.total_audio_secs.lock() {
            *secs += audio_secs;
        }
    }

    fn summary_line(&self) -> String {
        let count = self.dictation_count.load(Ordering::Relaxed);
        let secs = self.total_audio_secs.lock().map(|s| *s).unwrap_or(0.0);
        let label = if count == 1 {
            "dictation"
        } else {
            "dictations"
        };
        format!(
            "     {SUBTLE}session{RESET}  {DIM}{} {} {sep} {:.1}s audio{RESET}",
            count,
            label,
            secs,
            sep = sep()
        )
    }
}

// ── Spinner style helpers ────────────────────────────────────────────────────
//
// Inline status panel for the live dictation pipeline. Stays one line tall so
// the terminal remains usable around it. Palette and dot semantics live in
// `rekody_core::ui` and are shared with the history TUI.

/// Single shared style used for every state — avoids the new-line glitch
/// caused by swapping styles while `enable_steady_tick` is running.
fn spinner_style() -> ProgressStyle {
    ProgressStyle::with_template("  {msg}").unwrap()
}

fn set_spinner_msg(spinner: &ProgressBar, msg: impl Into<String>) {
    spinner.set_style(spinner_style());
    spinner.set_message(msg.into());
    spinner.tick();
}

fn set_idle_style(spinner: &ProgressBar) {
    // Each hotkey is a tight key→action pair; pairs are separated by a wider
    // gap with a brand-dim divider so it's obvious ⌥Space and Ctrl+C are
    // independent chords, not one combined sequence.
    let msg = format!(
        "{BRAND}◯{RESET}  {BRAND_LIGHT}{BOLD}rekody{RESET}    \
         {CREAM}{BOLD}⌥Space{RESET} {DIM}hold to dictate{RESET}    {sep}    \
         {CREAM}{BOLD}Ctrl+C{RESET} {DIM}quit{RESET}",
        sep = sep()
    );
    set_spinner_msg(spinner, msg);
}

fn set_recording_style(spinner: &ProgressBar, elapsed_secs: Option<f64>) {
    let msg = match elapsed_secs {
        Some(s) => format!(
            "{SLOW}{BOLD}●{RESET}  {SLOW}{BOLD}recording{RESET}  {sep}  {SLOW}{:.1}s{RESET}  {sep}  {DIM}release {RESET}{CREAM}{BOLD}⌥Space{RESET}{DIM} to stop{RESET}",
            s,
            sep = sep()
        ),
        None => format!(
            "{SLOW}{BOLD}●{RESET}  {SLOW}{BOLD}recording{RESET}  {sep}  {DIM}release {RESET}{CREAM}{BOLD}⌥Space{RESET}{DIM} to stop{RESET}",
            sep = sep()
        ),
    };
    set_spinner_msg(spinner, msg);
}

fn set_processing_style(spinner: &ProgressBar, detail: &str) {
    let msg = format!("{BRAND_LIGHT}{BOLD}◐{RESET}  {BRAND_LIGHT}{BOLD}{detail}{RESET}",);
    set_spinner_msg(spinner, msg);
}

fn set_done_style(spinner: &ProgressBar, text: &str, stt_ms: &str, llm_ms: Option<&str>) {
    let display = if text.len() > 60 {
        format!("{}…", &text[..59])
    } else {
        text.to_string()
    };
    let stt_num: u64 = stt_ms.parse().unwrap_or(0);
    let llm_num: u64 = llm_ms.and_then(|s| s.parse().ok()).unwrap_or(0);
    let total = stt_num + llm_num;
    let dot_color = latency_ansi(total);
    let lat = match llm_ms {
        Some(l) => format!("{stt_ms}ms STT {sep} {l}ms LLM", sep = sep()),
        None => format!("{stt_ms}ms STT"),
    };
    let msg = format!(
        "{OK}{BOLD}✓{RESET}  {CREAM}{}{RESET}  {sep}  {dot_color}●{RESET} {DIM}{}{RESET}",
        display,
        lat,
        sep = sep()
    );
    set_spinner_msg(spinner, msg);
}

fn set_error_style(spinner: &ProgressBar, msg: &str) {
    let short = if msg.len() > 70 { &msg[..70] } else { msg };
    let line = format!(
        "{SLOW}{BOLD}✗{RESET}  {SLOW}{}{RESET}  {sep}  {DIM}hold {RESET}{CREAM}{BOLD}⌥Space{RESET}{DIM} to retry{RESET}",
        short,
        sep = sep()
    );
    set_spinner_msg(spinner, line);
}

// ── Tracing → UI layer ───────────────────────────────────────────────────────

struct UiLayer {
    spinner: ProgressBar,
    session: Arc<SessionStats>,
    recording_start: Mutex<Option<Instant>>,
    stt_result: Mutex<Option<SttResult>>,
}

#[derive(Clone)]
struct SttResult {
    text: String,
    latency_ms: String,
    done_shown: bool,
}

impl UiLayer {
    fn new(spinner: ProgressBar, session: Arc<SessionStats>) -> Self {
        Self {
            spinner,
            session,
            recording_start: Mutex::new(None),
            stt_result: Mutex::new(None),
        }
    }

    fn on_recording_started(&self) {
        if let Ok(mut start) = self.recording_start.lock() {
            *start = Some(Instant::now());
        }
        set_recording_style(&self.spinner, None);
    }

    fn on_recording_stopped(&self) {
        // Show elapsed time as we transition to processing.
        let elapsed = self
            .recording_start
            .lock()
            .ok()
            .and_then(|g| g.map(|s| s.elapsed().as_secs_f64()));
        set_recording_style(&self.spinner, elapsed);
    }

    fn on_transcription_complete(&self, text: &str, latency_ms: &str) {
        if let Ok(mut guard) = self.stt_result.lock() {
            *guard = Some(SttResult {
                text: text.to_string(),
                latency_ms: latency_ms.to_string(),
                done_shown: false,
            });
        }
        set_processing_style(&self.spinner, "formatting with LLM…");
    }

    fn on_llm_complete(&self, llm_ms: &str) {
        let stt = self.stt_result.lock().ok().and_then(|mut g| {
            if let Some(ref mut r) = *g {
                r.done_shown = true;
            }
            g.clone()
        });
        if let Some(stt) = stt {
            set_done_style(&self.spinner, &stt.text, &stt.latency_ms, Some(llm_ms));
            self.record_and_show_stats();
        }
    }

    fn on_injected(&self) {
        let stt = self.stt_result.lock().ok().and_then(|g| g.clone());
        if let Some(ref stt) = stt
            && !stt.done_shown
        {
            set_done_style(&self.spinner, &stt.text, &stt.latency_ms, None);
            self.record_and_show_stats();
        }
        self.schedule_idle_reset();
    }

    fn on_error(&self, msg: &str) {
        set_error_style(&self.spinner, msg);
        self.schedule_idle_reset();
    }

    fn record_and_show_stats(&self) {
        let audio_secs = self
            .recording_start
            .lock()
            .ok()
            .and_then(|s| s.map(|start| start.elapsed().as_secs_f64()))
            .unwrap_or(0.0);
        self.session.record(audio_secs);
        self.spinner.println(self.session.summary_line());
    }

    fn schedule_idle_reset(&self) {
        let spinner = self.spinner.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(3));
            set_idle_style(&spinner);
        });
    }
}

impl<S> tracing_subscriber::Layer<S> for UiLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = EventVisitor::default();
        event.record(&mut visitor);
        let msg = &visitor.message;

        if msg.contains("recording started") {
            self.on_recording_started();
        } else if msg.contains("no speech detected") {
            self.on_error("no speech detected — speak louder or lower vad_threshold in config");
        } else if msg.contains("recording stopped") {
            self.on_recording_stopped();
        } else if msg.contains("received audio segment") {
            set_processing_style(&self.spinner, "transcribing…");
        } else if msg.contains("transcription complete") {
            let text = visitor.fields.get("text").cloned().unwrap_or_default();
            let latency = visitor
                .fields
                .get("latency_ms")
                .cloned()
                .unwrap_or_default();
            self.on_transcription_complete(&text, &latency);
        } else if msg.contains("LLM formatting complete") {
            let latency = visitor
                .fields
                .get("latency_ms")
                .cloned()
                .unwrap_or_default();
            self.on_llm_complete(&latency);
        } else if msg.contains("text injected successfully") {
            self.on_injected();
        } else if msg.contains("LLM formatting failed") || msg.contains("failed to process audio") {
            let err = visitor
                .fields
                .get("error")
                .cloned()
                .unwrap_or_else(|| msg.clone());
            self.on_error(&err);
        } else if msg.contains("empty transcript") {
            set_idle_style(&self.spinner);
        } else if msg.contains("no LLM API keys") {
            // Will show done on injection without LLM step.
        }
    }
}

// ── Tracing field visitor ────────────────────────────────────────────────────

#[derive(Default)]
struct EventVisitor {
    message: String,
    fields: std::collections::HashMap<String, String>,
}

impl tracing::field::Visit for EventVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        let val = format!("{:?}", value);
        let val = val.trim_matches('"').to_string();
        if field.name() == "message" {
            self.message = val;
        } else {
            self.fields.insert(field.name().to_string(), val);
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields
                .insert(field.name().to_string(), value.to_string());
        }
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.fields
            .insert(field.name().to_string(), value.to_string());
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.fields
            .insert(field.name().to_string(), format!("{:.1}", value));
    }
}
