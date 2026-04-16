# Changelog

All notable changes to yap-box are documented here. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); this project uses [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] — 2026-04-16

Initial public release. Apple Silicon (arm64) only.

### Added
- Tauri + React desktop app that sends text to a bundled Kokoros TTS sidecar and plays the result through `afplay`.
- Drag-and-drop support for `.txt` and `.md` files up to 1 MiB.
- First-launch gate that downloads the Kokoro-82M model (~354 MB) only after explicit user confirmation (two-click flow).
- Voice selection persisted to `localStorage`; live voice list pulled from Kokoros with a hardcoded fallback.
- Cmd+Enter keyboard shortcut to speak the current text.
- Ad-hoc signed `.app` + DMG release pipeline in `.github/workflows/release.yml`, building the Kokoros sidecar from a pinned upstream commit SHA.

[Unreleased]: https://github.com/ChristianAlexa/yap-box/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/ChristianAlexa/yap-box/releases/tag/v0.1.0
