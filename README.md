# yap-box

Drop text in, hear it spoken. A small Tauri app that sends text to a local [Kokoros](https://github.com/lucasjinreal/Kokoros) TTS engine and plays the audio.

Part of the yap family — [yap](https://github.com/ChristianAlexa/yap) is the MCP bridge for Claude Desktop; yap-box is a standalone GUI for any text.

## What it does

- Paste text into the textarea and press **Yap**
- Drag and drop a `.txt` or `.md` file onto the window to load its contents
- Audio plays through your system speakers via `afplay`

## Install

1. **Download the latest `.dmg`** from the [Releases page](https://github.com/ChristianAlexa/yap-box/releases). Apple Silicon (arm64) only for now — pick the asset named `yap-box_*_aarch64.dmg`.
2. **Move `yap-box.app` to `/Applications`.**
3. **Clear the quarantine flag** so macOS stops blocking the unsigned build. Pick whichever you prefer:
   ```bash
   xattr -dr com.apple.quarantine /Applications/yap-box.app
   ```
   Or, without a terminal: right-click `yap-box.app` → **Open** → **Open** in the warning dialog. On macOS 15 (Sequoia) right-click-Open may not bypass Gatekeeper — open **System Settings → Privacy & Security** and click **Open Anyway** next to the blocked-app message.
4. **Start Kokoros before using yap-box** — see Prerequisites below. yap-box does not bundle Kokoros.

## Prerequisites

- macOS (required — audio playback uses `afplay`, a macOS-only binary, with no fallback for Linux/Windows)
- Node ≥ 20.11
- Rust ≥ 1.77.2
- [Kokoros](https://github.com/lucasjinreal/Kokoros) running on `localhost:3000` (`koko openai`)

## Development

```bash
npm install
npm run dev:tauri
```

## Kokoros binary discovery

On startup, the Rust backend checks whether Kokoros is already running on `localhost:3000`. If not, it tries to auto-spawn `koko openai`, resolving the binary in this order:

1. `$KOKOROS_BINARY` (if set and the path exists)
2. `~/dev/Kokoros/target/release/koko`
3. `koko` on `$PATH`

If none of the three resolve, the app logs an error to stderr and continues running — you'll just need to start Kokoros yourself.

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

**Red banner "Kokoros not running"**
yap-box talks to a Kokoros server on `localhost:3000`. Either Kokoros isn't running, or it's on a different port. Start it with `koko openai` in a terminal and leave it running. See [Kokoros binary discovery](#kokoros-binary-discovery) for how yap-box finds the binary on startup.

**Voices dropdown is empty or only shows defaults**
yap-box fetches the live voice list from Kokoros (`/v1/audio/voices`) with a 1.5-second timeout. If Kokoros is slow to start or the endpoint isn't available in your Kokoros version, it falls back to a hardcoded list. Restart Kokoros, then relaunch yap-box.

**Drag-and-drop does nothing**
yap-box only accepts `.txt` and `.md` files up to 1 MiB. Check the file extension and size.

## Third-party components

yap-box is MIT-licensed and depends on software it does not bundle:

- [Kokoros](https://github.com/lucasjinreal/Kokoros) — Apache License 2.0
- [Kokoro-82M](https://huggingface.co/hexgrad/Kokoro-82M) model — Apache License 2.0

Install and run Kokoros yourself per its instructions. yap-box only talks to it over HTTP on `localhost:3000`.

## License

[MIT](LICENSE) © Christian Alexa
