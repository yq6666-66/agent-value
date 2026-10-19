# agent-value

A local, personal desktop tool (planned as a tray-resident app) that will score
the (model, route) combinations one user actually uses on cost, time and
quality, with weights taken from that user's own preferences. It is not a
public leaderboard.

## Current state

- A design specification exists (kept outside this repository).
- This repository is a skeleton. **Nothing is scored yet**; no formula is
  implemented, and there is no data storage, probing or UI beyond a placeholder page.

## Layout

| Path | What it is |
|---|---|
| `crates/agent-value-core/` | Pure-Rust core library. All formula logic and its tests will live here. No Tauri or GUI dependencies. |
| `src-tauri/` | Minimal Tauri 2 desktop shell that links the core crate. Not a member of the default Cargo workspace. |
| `web/` | React + TypeScript + Vite front end. Display only; it never computes scores. |

## Building

Core crate (no system GUI libraries needed):

```sh
cargo fmt --all --check
cargo clippy -p agent-value-core --all-targets -- -D warnings
cargo test -p agent-value-core
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
first, then run `cargo build` from inside `src-tauri/`. The shell is not built
in CI yet.

## Secrets

Real API keys go only in a local `.env` file, which is git-ignored.
`.env.example` lists variable names only. agent-value stores the name of the
environment variable a route uses, never its value.

## License

Apache-2.0. See `LICENSE` and `THIRD_PARTY_NOTICES`.
