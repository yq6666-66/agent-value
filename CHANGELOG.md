# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
- Repository skeleton: Cargo workspace with the pure-Rust `agent-value-core` crate,
  a minimal Tauri 2 shell in `src-tauri/` (outside the default workspace),
  and a minimal React + TypeScript + Vite display app in `web/`.
- CI workflow: rustfmt, clippy (`-D warnings`) and tests for the core crate;
  `npm ci`, lint, type check and build for the web app.
- Apache-2.0 license, empty third-party notices, `.env.example` with variable names only.
