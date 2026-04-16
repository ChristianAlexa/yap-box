# yap-box

Drop text in, hear it spoken. A small Tauri app that sends text to a local [Kokoros](https://github.com/lucasjinreal/Kokoros) TTS engine and plays the audio.

Part of the yap family — [yap](https://github.com/ChristianAlexa/yap) is the MCP bridge for Claude Desktop; yap-box is a standalone GUI for any text.

## What it does

- Paste text into the textarea and press **Yap**
- Drag and drop a `.txt` or `.md` file onto the window to load its contents
- Audio plays through your system speakers via `afplay`

## Install

> **Requires an Apple Silicon Mac (M1/M2/M3/M4).** yap-box ships an arm64-only binary — Intel Macs are not supported. Check under `About This Mac`; "Chip" should start with "Apple".

1. **Download the latest `.dmg`** from the [Releases page](https://github.com/ChristianAlexa/yap-box/releases). Apple Silicon (arm64) only for now — pick the asset named `yap-box_*_aarch64.dmg`.
2. **Move `yap-box.app` to `/Applications`.**
3. **Clear the quarantine flag** so macOS stops blocking the unsigned build. Pick whichever you prefer:
   ```bash
   xattr -dr com.apple.quarantine /Applications/yap-box.app
   ```
   Or, without a terminal: right-click `yap-box.app` → **Open** → **Open** in the warning dialog. On macOS 15 (Sequoia) right-click-Open may not bypass Gatekeeper — open **System Settings → Privacy & Security** and click **Open Anyway** next to the blocked-app message.
4. **On first launch, yap-box will prompt you to download the Kokoro-82M TTS model** (~354 MB). No bytes are fetched until you click **Download** in the confirmation dialog. The model is stored at `~/Library/Application Support/com.yapbox.app/models/`. Kokoros itself is bundled with the app — you don't need to install it separately.

## Prerequisites (for building from source)

- macOS (required — audio playback uses `afplay`, a macOS-only binary, with no fallback for Linux/Windows)
- Node ≥ 20.11
- Rust ≥ 1.77.2
- A local [Kokoros](https://github.com/lucasjinreal/Kokoros) build at `src-tauri/binaries/koko-aarch64-apple-darwin` for `npm run dev:tauri` (copy from `~/dev/Kokoros/target/release/koko` if you have it locally; CI builds this from a pinned upstream SHA)

## Development

```bash
npm install
npm run dev:tauri
```

## How Kokoros is bundled

yap-box ships `koko` as a Tauri sidecar at `src-tauri/binaries/koko-<target-triple>`. On launch, after the model is present, yap-box allocates a random localhost port, spawns the sidecar pointing at the downloaded model files, and waits for `/v1/audio/voices` to answer before enabling the UI. The sidecar is killed on app exit.

CI builds the sidecar from a pinned upstream commit SHA (see `.github/workflows/release.yml`) and ad-hoc signs it alongside the .app so Gatekeeper only prompts once.

## Build

```bash
npm run build:tauri
```

## How it relates to yap

Both apps are thin clients of the same Kokoros TTS service. yap speaks to Claude Desktop over MCP; yap-box speaks to you through a window. Neither depends on the other — they're peers.

## Troubleshooting

**App won't launch / "unidentified developer" warning**
Remove the quarantine attribute and try again:
```bash
xattr -dr com.apple.quarantine /Applications/yap-box.app
```

**"Kokoros failed to start"**
The bundled sidecar couldn't spawn or the health probe timed out. Check the Console app for logs tagged `[koko]`, or launch yap-box from a terminal (`open /Applications/yap-box.app`) to see stderr. Common causes: model files corrupted mid-download (re-trigger from an empty state by deleting `~/Library/Application Support/com.yapbox.app/models/`), or a Gatekeeper denial on the sidecar (re-run `xattr -dr com.apple.quarantine /Applications/yap-box.app`).

**Voices dropdown is empty or only shows defaults**
yap-box fetches the live voice list from Kokoros (`/v1/audio/voices`) with a 1.5-second timeout. If Kokoros is slow to start or the endpoint isn't available in your Kokoros version, it falls back to a hardcoded list. Restart Kokoros, then relaunch yap-box.

**Drag-and-drop does nothing**
yap-box only accepts `.txt` and `.md` files up to 1 MiB. Check the file extension and size.

## Third-party components

yap-box is MIT-licensed and bundles or downloads the following:

- [Kokoros](https://github.com/lucasjinreal/Kokoros) — Apache License 2.0 — **bundled as a sidecar binary** in the .dmg
- [Kokoro-82M](https://huggingface.co/hexgrad/Kokoro-82M) model — Apache License 2.0 — **downloaded on first launch** (with user confirmation) from [`thewh1teagle/kokoro-onnx` releases](https://github.com/thewh1teagle/kokoro-onnx/releases) (a mirror of the model in ONNX format)
- [espeak-ng](https://github.com/espeak-ng/espeak-ng) — GPL v3 — **statically linked into `koko`** and its phoneme data files bundled under `Contents/Resources/` (required for Kokoros to convert text into phonemes)

yap-box communicates with the bundled Kokoros over HTTP on a random localhost port allocated at launch.

See [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md) for the full license texts (Apache 2.0 and GPL v3) covering these components.

## Privacy

yap-box runs entirely on your machine — no analytics, telemetry, or remote servers. See [PRIVACY.md](PRIVACY.md) for details.

## License

[MIT](LICENSE) © Christian Alexa
