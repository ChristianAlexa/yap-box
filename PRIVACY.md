# Privacy Policy

yap-box is a local-only desktop app. It does not collect, transmit, or store any personal data on remote servers.

## What the app does

- Reads text you type, paste, or drop into the window
- Sends that text to the bundled Kokoros TTS sidecar running on a randomly-allocated `127.0.0.1` port on your own machine
- Receives synthesized audio from Kokoros and plays it through your system speakers via `afplay`
- Writes temporary `.wav` files to `/tmp` and deletes them after playback

## What the app does not do

- No analytics, telemetry, or crash reporting
- No microphone or camera access
- No account, login, or user identifier
- No background data collection

## One-time model download

On first launch, yap-box prompts you to download the Kokoro-82M TTS model (~354 MB) from a public GitHub release. The download only begins after you explicitly confirm. Once the model is stored locally at `~/Library/Application Support/com.yapbox.app/models/`, no further network requests are made to any remote server.

## Third-party services

yap-box depends on [Kokoros](https://github.com/lucasjinreal/Kokoros), which you install and run locally. Text you synthesize is handled by that local process according to its own behavior — it does not leave your machine through yap-box.

## Contact

Questions: open an issue on the project repository.
