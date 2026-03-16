# Chamgei

**An open-source, privacy-first voice dictation system.**

Chamgei turns your voice into text anywhere on your desktop. It runs a local Whisper model for speech-to-text, optionally polishes the output through fast cloud LLMs (Groq, Cerebras), and injects the result at your cursor -- all behind a single hotkey.

## Features

- **Local speech-to-text** -- On-device Whisper inference via whisper.cpp (Metal GPU acceleration on macOS).
- **LLM formatting** -- Optional post-processing through Cerebras or Groq to clean filler words, add punctuation, and fix grammar.
- **Context-aware** -- Detects the active application and adapts formatting (e.g., preserves code identifiers in editors, uses casual tone in chat apps).
- **Command mode** -- Select text, speak an instruction ("make this more concise", "translate to Spanish"), and have an LLM transform it in place.
- **Cross-platform** -- macOS, Windows, and Linux support with platform-native text injection.
- **Offline mode** -- Works fully offline with local Whisper; LLM formatting is skipped gracefully when no API keys are configured.
- **Privacy-first** -- Audio never leaves your machine. LLM calls send only the transcript text, and only when you opt in.

## Quick Start

### Prerequisites

- [Rust](https://rustup.rs/) (edition 2024, stable toolchain)
- [Node.js](https://nodejs.org/) >= 18 (for the Tauri frontend)
- A Whisper GGML model file (see step 3 below)

### Build & Run

```bash
# 1. Clone the repository
git clone https://github.com/tonykipkemboi/chamgei.git
cd chamgei

# 2. Install frontend dependencies
npm install

# 3. Download a Whisper model (small, English-only, ~250 MB)
mkdir -p ~/.local/share/chamgei/models
curl -L -o ~/.local/share/chamgei/models/ggml-small.en.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin

# 4. Build and run
cargo tauri dev
```

For a production build:

```bash
cargo tauri build
```

## Usage

### Hotkeys

| Action | macOS | Windows / Linux |
|---|---|---|
| Dictate (push-to-talk) | `Cmd+Shift+Space` (hold) | `Ctrl+Shift+Space` (hold) |
| Dictate (toggle mode) | `Cmd+Shift+Space` (press) | `Ctrl+Shift+Space` (press) |
| Command mode | `Cmd+Shift+Enter` | `Ctrl+Shift+Enter` |

- **Push-to-talk** (default): hold the hotkey while speaking, release to transcribe.
- **Toggle**: press once to start recording, press again to stop and transcribe.
- **Command mode**: select text first, press the hotkey, speak your instruction, and the selected text is replaced with the LLM result.

### Settings

The settings window is accessible from the system tray icon. You can configure the activation mode, LLM provider, Whisper model size, VAD sensitivity, and text injection method.

## Architecture

```
 Hotkey ──> Audio Capture ──> VAD ──> STT (Whisper) ──> LLM (optional) ──> Text Injection
   |             |              |          |                  |                   |
   v             v              v          v                  v                   v
 rdev        cpal/rubato    energy-    whisper-rs       Cerebras/Groq     clipboard paste
             16kHz mono      based                      provider chain    or native input
```

The project is organized as a Cargo workspace with the following crates:

| Crate | Description |
|---|---|
| `chamgei-core` | Pipeline orchestration, configuration, context detection, prompt management |
| `chamgei-audio` | Microphone capture via cpal, resampling to 16kHz mono via rubato, energy-based VAD |
| `chamgei-stt` | Speech-to-text engines (local Whisper via whisper-rs) |
| `chamgei-llm` | LLM post-processing providers (Cerebras, Groq, local llama.cpp stub, raw fallback) with automatic failover chain |
| `chamgei-inject` | Cross-platform text injection (clipboard paste, native keyboard simulation) |
| `chamgei-hotkey` | Global hotkey listener via rdev (push-to-talk and toggle modes) |
| `src-tauri` | Tauri desktop app shell, system tray, settings UI |

The frontend is a React + TypeScript app built with Vite and styled with Tailwind CSS.

## Configuration

Chamgei reads its configuration from `~/.config/chamgei/config.toml`. Copy the default config to get started:

```bash
mkdir -p ~/.config/chamgei
cp config/default.toml ~/.config/chamgei/config.toml
```

Key settings:

```toml
# Activation mode: "push_to_talk" or "toggle"
activation_mode = "push_to_talk"

# LLM provider: "cerebras", "groq", or "local"
llm_provider = "cerebras"

# API keys (uncomment and fill in to enable LLM formatting)
# cerebras_api_key = "csk-..."
# groq_api_key = "gsk_..."

# Whisper model: "tiny", "small", "medium", "large"
whisper_model = "small"

# VAD sensitivity (0.0 = very sensitive, 1.0 = least sensitive)
vad_threshold = 0.5

# Text injection: "clipboard" or "native"
injection_method = "clipboard"
```

The model file is resolved in this order:
1. `$CHAMGEI_MODEL_DIR/<model-file>`
2. `~/.local/share/chamgei/models/<model-file>`
3. `./models/<model-file>`

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, project structure, and how to submit changes.

## License

Chamgei is dual-licensed under [MIT](LICENSE) or [Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0), at your option.
