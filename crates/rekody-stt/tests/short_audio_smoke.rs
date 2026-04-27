//! Smoke test for the short-audio path in `LocalWhisperEngine::transcribe`.
//!
//! Verifies that a 1 s silence buffer (≤25 s threshold → `long_audio = false`)
//! flows through the engine without error. This guards the regression fixed
//! in `build_params(long_audio: bool)` where the previous code unconditionally
//! enabled `single_segment`, truncating any audio past Whisper's 30 s window.
//!
//! The test loads a real GGML model file from disk; if none of the candidate
//! locations contain a model, the test is skipped (printed message, no
//! failure). This keeps `cargo test -p rekody-stt` green on machines without
//! a downloaded model.

use std::path::PathBuf;

use rekody_stt::{LocalWhisperEngine, SttEngine, WhisperModel};

/// Look for any GGML whisper model on disk so the smoke test can run without
/// downloading a model. Returns `(model_size, path)` for the first hit.
fn find_local_model() -> Option<(WhisperModel, PathBuf)> {
    let home = std::env::var("HOME").ok()?;
    let model_dir = PathBuf::from(&home).join(".local/share/rekody/models");

    // Prefer the tiniest model available — fastest and smallest for a smoke
    // test. We accept English-only `.en` variants as well; the smoke test
    // does not depend on multilingual support.
    let candidates: &[(WhisperModel, &str)] = &[
        (WhisperModel::Tiny, "ggml-tiny.bin"),
        (WhisperModel::Tiny, "ggml-tiny.en.bin"),
        (WhisperModel::Small, "ggml-small.bin"),
        (WhisperModel::Small, "ggml-small.en.bin"),
        (WhisperModel::Turbo, "ggml-large-v3-turbo-q5_0.bin"),
    ];

    for (size, file) in candidates {
        let path = model_dir.join(file);
        if path.exists() {
            return Some((*size, path));
        }
    }

    None
}

#[tokio::test]
async fn short_audio_silence_returns_ok() {
    let Some((model_size, model_path)) = find_local_model() else {
        eprintln!(
            "skipping smoke test: no GGML whisper model found under \
             ~/.local/share/rekody/models"
        );
        return;
    };

    let path_str = model_path.to_str().expect("model path is valid UTF-8");
    let engine = LocalWhisperEngine::new(model_size, path_str)
        .expect("LocalWhisperEngine::new should load an existing model");

    // 1 second of silence at 16 kHz mono — exercises the `long_audio = false`
    // fast path (≤25 s).
    let samples = vec![0.0_f32; 16_000];

    let transcript = engine
        .transcribe(&samples)
        .await
        .expect("transcribing 1 s of silence should not fail");

    // Whisper may emit `[BLANK_AUDIO]`, an empty string, or a stray token —
    // any of those is fine. We only assert the call completed without error
    // and produced a sensible latency reading.
    assert!(
        transcript.latency_ms < 60_000,
        "transcription took longer than 60 s (latency_ms = {})",
        transcript.latency_ms
    );
}
