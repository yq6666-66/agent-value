# agent-value

A local, personal desktop tool (planned as a tray-resident app) that will score
the (model, route) combinations one user actually uses on cost, time and
quality, with weights taken from that user's own preferences. It is not a
public leaderboard.

## Current state

- A design specification exists (kept outside this repository).
- This repository is an early skeleton. **Nothing is scored yet.**
- The core crate has the foundations only: the specification's numeric
  constants, scoring scopes (coding / non-coding), arms as (model, route),
  time windows with freezing, and weight versions including trial versions.
  These are covered by unit tests.
- The core crate also judges task outcomes (success / failure / cancel /
  undetermined, with provisional results while the 30-minute redo and
  24-hour rollback windows are open) and computes the rating-based quality
  signals, switching a whole layer to "delivery success rate" when ratings
  are too few. Nothing consumes these yet.
- The store crate has the local SQLite data model: route metadata per
  request, tasks, behaviour events with available-arm snapshots, probe
  results, frozen cell weights, weight versions and other configuration
  versions, a history of outcome judgements, with the privacy rules below. It is not wired into the desktop
  shell yet.
- No cost, time or quality formula, no value score, no probing, and no UI
  beyond a placeholder page exist yet.

## Layout

| Path | What it is |
|---|---|
| `crates/agent-value-core/` | Pure-Rust core library. All formula logic and its tests will live here. No dependencies. |
| `crates/agent-value-store/` | Local SQLite storage (bundled SQLite, no system library needed). |
| `src-tauri/` | Minimal Tauri 2 desktop shell that links the core crate. Not a member of the default Cargo workspace. |
| `web/` | React + TypeScript + Vite front end. Display only; it never computes scores. |

## Building

Core and store crates (no system GUI libraries needed; a C compiler is
needed for the bundled SQLite):

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Web app (Node.js 20+):

```sh
cd web
npm ci
npm run lint
npm run typecheck
npm run build
```

Desktop shell: `src-tauri/` is a separate Cargo package so that the core can be
tested without GUI libraries. Building it needs the Tauri 2 system prerequisites
(on Linux: WebKitGTK 4.1 and related development packages). Build the web app
first, then run `cargo build` from inside `src-tauri/`. CI installs these
prerequisites and runs `cargo check` and `cargo clippy` for the shell.

## Secrets and privacy

Real API keys go only in a local `.env` file, which is git-ignored.
`.env.example` lists variable names only. agent-value stores the name of the
environment variable a route uses, never its value; the database has no
column for key values.

All data stays in one local SQLite file. Prompt and code text is never
stored: only a category, a length and an HMAC-SHA256 digest whose key is
kept outside the database. Task-level data is purged after 180 days, and
the store can export every row or delete everything.

## License

Apache-2.0. See `LICENSE` and `THIRD_PARTY_NOTICES`.
