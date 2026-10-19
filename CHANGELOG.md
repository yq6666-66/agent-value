# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
- Store, spec step 2: new `agent-value-store` crate with a versioned SQLite
  schema for arms, tasks, per-request route metadata, behaviour events with
  available-arm snapshots, probe results (Beijing-time run days), frozen cell
  weights, weight versions with frozen windows, and configuration versions.
  Version rows are immutable (triggers). Privacy: no key-value column, user
  text kept only as category + length + keyed digest, 180-day purge, export
  and delete-all (with WAL checkpoint and VACUUM).
- Core, spec step 2: design cells with the reserved probe cell (weight
  always 0), cell weights, privacy checks (env var names, secret-like
  values, text facts), run days, data-model records, integer money
  (micro-yuan), and ledger reconstruction from stored parts.
- CI tests and lints the whole Cargo workspace.
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
