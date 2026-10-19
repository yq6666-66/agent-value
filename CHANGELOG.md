# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
- Core, spec step 1: all v22 §9 constants (checked row by row against a verbatim
  copy of the §9 table); scoring scopes (default coding / non-coding, gated
  subdivision); arms as (model, route) with route = provider kind + host +
  protocol + client; fixed-length time windows with per-window freezing;
  weight versions with source, evidence, effective window and trial versions;
  first-window rule (no V). All bad input returns `Result` errors.
- Repository skeleton: Cargo workspace with the pure-Rust `agent-value-core` crate,
  a minimal Tauri 2 shell in `src-tauri/` (outside the default workspace),
  and a minimal React + TypeScript + Vite display app in `web/`.
- CI workflow: rustfmt, clippy (`-D warnings`) and tests for the core crate;
  `npm ci`, lint, type check and build for the web app; Tauri 2 Linux system
  dependencies plus `cargo check` and `cargo clippy` for the desktop shell.
- Apache-2.0 license, empty third-party notices, `.env.example` with variable names only.
