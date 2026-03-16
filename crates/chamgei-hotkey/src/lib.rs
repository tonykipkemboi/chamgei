//! Global hotkey listener for Chamgei voice dictation.
//!
//! Registers system-wide keyboard shortcuts and emits events
//! for recording start/stop via push-to-talk or toggle mode.
//!
//! ## Hotkey bindings
//!
//! | Action | Shortcut |
//! |--------|----------|
//! | Push-to-talk (hold to record, release to stop) | `Fn` |
//! | Hands-free toggle (press to start, press to stop) | `Fn + Space` |
//! | Command mode (transform selected text) | `Fn + Enter` |

use anyhow::Result;
use rdev::{Event, EventType, Key, listen};
use std::sync::{Arc, Mutex};
use thiserror::Error;
use tokio::sync::mpsc;

#[derive(Debug, Error)]
pub enum HotkeyError {
    #[error("failed to register hotkey: {0}")]
    Registration(String),
    #[error("hotkey listener error: {0}")]
    Listener(String),
}

/// Events emitted by the hotkey listener.
#[derive(Debug, Clone)]
pub enum HotkeyEvent {
    /// User wants to start recording.
    RecordStart,
    /// User wants to stop recording and trigger transcription.
    RecordStop,
    /// User activated command mode (select text + voice instruction).
    CommandMode,
}

/// Activation mode for the dictation hotkey.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationMode {
    /// Hold Fn to record, release to transcribe.
    PushToTalk,
    /// Fn+Space to start, Fn+Space again to stop.
    Toggle,
}

/// Configuration for the hotkey listener.
#[derive(Debug, Clone)]
pub struct HotkeyConfig {
    pub activation_mode: ActivationMode,
}

impl Default for HotkeyConfig {
    fn default() -> Self {
        Self {
            activation_mode: ActivationMode::PushToTalk,
        }
    }
}

/// Tracks the state of the Fn key and recording.
#[derive(Debug, Default)]
struct KeyState {
    fn_pressed: bool,
    /// Whether we are currently recording (used for both modes).
    is_recording: bool,
    /// In push-to-talk, tracks if Fn alone triggered the recording
    /// (as opposed to Fn+Space or Fn+Enter which are different actions).
    fn_only: bool,
}

/// Returns true if the given key is the Fn/Globe key.
///
/// On macOS the Fn key is reported as `Key::Function` by rdev.
/// On other platforms, we fall back to `Key::Function` as well,
/// though availability varies by keyboard/driver.
fn is_fn_key(key: &Key) -> bool {
    matches!(key, Key::Function)
}

/// Starts the global hotkey listener and returns a receiver for hotkey events.
///
/// ## Hotkey bindings
///
/// - **Push-to-talk**: Hold `Fn` to record, release to stop and transcribe
/// - **Hands-free toggle**: `Fn + Space` to start/stop recording
/// - **Command mode**: `Fn + Enter` to transform selected text by voice
///
/// The `rdev` listener runs on a dedicated OS thread because it blocks.
/// Events are sent through a `tokio::sync::mpsc::UnboundedSender`.
pub fn start_listener(config: HotkeyConfig) -> Result<mpsc::UnboundedReceiver<HotkeyEvent>> {
    let (tx, rx) = mpsc::unbounded_channel();

    let state = Arc::new(Mutex::new(KeyState::default()));
    let activation_mode = config.activation_mode;

    std::thread::spawn(move || {
        tracing::info!("hotkey listener started (mode: {:?})", activation_mode);
        tracing::info!("bindings: Fn = push-to-talk, Fn+Space = toggle, Fn+Enter = command mode");

        let callback = move |event: Event| {
            let mut state = match state.lock() {
                Ok(s) => s,
                Err(poisoned) => poisoned.into_inner(),
            };

            match event.event_type {
                EventType::KeyPress(key) => {
                    if is_fn_key(&key) {
                        state.fn_pressed = true;
                        state.fn_only = true; // assume Fn-only until another key is pressed

                        // In push-to-talk mode, Fn alone starts recording
                        if activation_mode == ActivationMode::PushToTalk && !state.is_recording {
                            state.is_recording = true;
                            tracing::debug!("push-to-talk: RecordStart (Fn pressed)");
                            let _ = tx.send(HotkeyEvent::RecordStart);
                        }
                        return;
                    }

                    // If Fn is held and another key is pressed, it's a combo
                    if state.fn_pressed {
                        state.fn_only = false; // no longer Fn-only

                        match key {
                            // --- Fn + Space: toggle hands-free recording ---
                            Key::Space => {
                                // If we were in push-to-talk recording from Fn alone,
                                // cancel it — the user wants toggle mode instead.
                                if activation_mode == ActivationMode::PushToTalk && state.is_recording {
                                    // Switch behavior: Fn+Space always toggles
                                    state.is_recording = false;
                                    tracing::debug!("push-to-talk cancelled, Fn+Space = toggle");
                                    let _ = tx.send(HotkeyEvent::RecordStop);
                                }

                                // Toggle recording on/off
                                if state.is_recording {
                                    state.is_recording = false;
                                    tracing::debug!("toggle: RecordStop (Fn+Space)");
                                    let _ = tx.send(HotkeyEvent::RecordStop);
                                } else {
                                    state.is_recording = true;
                                    tracing::debug!("toggle: RecordStart (Fn+Space)");
                                    let _ = tx.send(HotkeyEvent::RecordStart);
                                }
                            }

                            // --- Fn + Enter: command mode ---
                            Key::Return => {
                                // Cancel any in-progress push-to-talk recording
                                if state.is_recording && activation_mode == ActivationMode::PushToTalk {
                                    state.is_recording = false;
                                    let _ = tx.send(HotkeyEvent::RecordStop);
                                }
                                tracing::debug!("command mode activated (Fn+Enter)");
                                let _ = tx.send(HotkeyEvent::CommandMode);
                            }

                            _ => {}
                        }
                    }
                }

                EventType::KeyRelease(key) => {
                    if is_fn_key(&key) {
                        // In push-to-talk mode, releasing Fn stops recording
                        // (only if it was a Fn-only press, not Fn+Space)
                        if activation_mode == ActivationMode::PushToTalk
                            && state.is_recording
                            && state.fn_only
                        {
                            state.is_recording = false;
                            tracing::debug!("push-to-talk: RecordStop (Fn released)");
                            let _ = tx.send(HotkeyEvent::RecordStop);
                        }

                        state.fn_pressed = false;
                        state.fn_only = false;
                    }
                }

                _ => {}
            }
        };

        if let Err(e) = listen(callback) {
            tracing::error!("hotkey listener failed: {:?}", e);
        }
    });

    Ok(rx)
}
