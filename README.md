# agent-value

[简体中文](README.zh-CN.md)

A local desktop tool for personal use (Tauri; build from source; no install package today). It scores the (model, route) pairs you actually use on cost, time, and quality, with weights set to your own preferences. It is not a public leaderboard: numbers come from your own tasks, and data stays on your machine. Scoring follows out-of-repo spec v22 and decision records; this README covers only what is implemented.

## Problem

Public boards and generic benchmarks do not match the models and routes you use. You need on-device comparison of arms (model + route) on value and authenticity signals, driven by your own tasks and preferences, without sending the data away.

## Features

1. **Coding-scope window scoring**: frontier, eligibility, and recommendation; shows authenticity and weight sources. The first window may show the frontier labeled “not yet calibrated” and omits V. Tier thresholds stay provisional; authenticity is unsettled and for reference only. Observation runs and full-window calibration are permanently cancelled.
2. **Arms**: list / add; show database path; export; clear all records.
3. **Five-level self-report wizard** (weight elicitation); sensitivity panel (winner chart, robustness labels, flip sentences; chart hidden on the first window or when there is no anchor layer).
4. **Probe**: records after consent; shows recorded / replay results; after consent you can run once manually; optional in-app timed reruns (off by default; after enable, waits a provisional interval before the first send; stops when the app quits; not OS cron; opening the app does not send; monthly budget stop is reflected in the panel).
5. **Fingerprints**: the library can collect official direct-connection fingerprint baselines over HTTP into the local store (tests may use a mock). The desktop UI has no one-click “collect baseline” button.

## Technology

1. Rust core library + local SQLite store + Tauri desktop shell; the web view does not recompute scores (the shell reads the store and asks core).
2. Probe HTTP path and fingerprint collection library are landed; secrets use environment variable names only and never enter the repo.
3. Spec steps 1–13 on the judgment side are implemented.

## Usage

1. Build and run the desktop app from source. Build the web frontend first (optional examples: `npm ci` / `npm run build`), then `cargo build` under `src-tauri/` (its own Cargo package), or use `cargo tauri dev` for local development and run the resulting binary. Tauri 2 system dependencies are required (on Linux: WebKitGTK 4.1, etc.). No install package is provided.
2. In the app, manage arms and run self-report / sensitivity; after consent, run a probe once manually or view replay, or enable in-app timed reruns (see Features).
3. Optional examples: `cargo test --workspace` for the core; `cargo run -p agent-value-core --example probe_plan` / `probe_judge` for probes.
4. Version history lives in `CHANGELOG.md`; this README has none.
