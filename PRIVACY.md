# Privacy Policy

yap-box is a local-only desktop app. It does not collect, transmit, or store any personal data on remote servers.

## What the app does

- Reads text you type, paste, or drop into the window
- Sends that text to a **local** Kokoros TTS server at `http://localhost:3000` on your own machine
- Receives synthesized audio from Kokoros and plays it through your system speakers via `afplay`
- Writes temporary `.wav` files to `/tmp` and deletes them after playback

## What the app does not do

- No analytics, telemetry, or crash reporting
- No network requests to any remote server
- No microphone or camera access
- No account, login, or user identifier
- No background data collection

## Third-party services

yap-box depends on [Kokoros](https://github.com/lucasjinreal/Kokoros), which you install and run locally. Text you synthesize is handled by that local process according to its own behavior — it does not leave your machine through yap-box.

## Contact

Questions: open an issue on the project repository.
