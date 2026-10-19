# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
- Core, spec steps 4-5: Beta CDF and quantile (own implementation, checked
  against 40-digit references); prior `p_B` / `kappa` exactly as written
  (`rho = 1` direct pooling as its own case, `0/0` gives 0); posterior,
  `q_K` then `p_K = 1 - q_K`, `E[n_F]` by geometric sum (no cancellation
  at tiny `pi`), `E[n_W]`; lower-bound fail test via the CDF, quantile for
  display; cancel-robust variant; cell states miss / F_thin / F_bad / valid
  over the positive-weight cells with counts and weight sums; `omega_u`
  from the previous window's user-task shares with carry-over / equal
  fallbacks.
- Core, spec step 3: task outcome judgement (§3.1 rules 1-6 in order:
  explicit abort, failed last check, redo within 30 min, whole rollback
  within 24 h for coding tasks, reported completion, else undetermined),
  with provisional results until the observation windows close; quality
  signals: star-to-quality mapping, per-layer rating / success-rate mode,
  pooled prior `q_B`, shrunk `q_hat`, spot-rating decision (10%, max 3 per
  Beijing-time day).
- Store, spec step 3: schema version 2 with an append-only
  `outcome_judgements` history (rule fired, rules version); recording a
  judgement updates the task's current outcome; replaceable task ratings.
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
