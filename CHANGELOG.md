# Changelog

All notable changes to this project are documented here.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

### Changed

### Fixed

## [0.1.0] - 2026-10-23

### Added
- Desktop window: the placeholder page shows `CORE_VERSION` from the Tauri
  `core_version` command in an About area. The string is whatever the shell
  returns; the web UI does not compute it.
- Desktop window: `demo_example6_layer` runs the example-6 diagnostic path
  in the core and the page shows the returned `A_C`. The card is labelled
  as a demo fixture, not the user's data.
- Desktop window: the shell opens the local store and the page can list and
  add arms, show the database path, export, and delete all rows. Arm rows
  have no key column. Export is the store dump, which keeps a digest instead
  of prompt text.
- Desktop window: a resizable frontier pane and a weight panel. The panel
  reads sample window chrome from the core. A first window shows
  `FIRST_WINDOW_LABEL` and does not show V. A later window has an empty V
  slot. User scoring is still not wired up.
- README (English and Chinese): section 11, probes and relay checks (§11),
  with section 6 gate 4, the roadmap rows, constants, the local probe
  commands and a `tools/` row in the layout table (the runner's tests run
  in CI against a local mock).
- Core, spec step 13: probes and relay detection (§11, §5 rule 4, §9
  budget), new modules `probe` and `authenticity`. The core sends no
  request: probe runs are judged from recorded exchanges in a fixed
  tab-separated replay format.
  Deterministic set: 10 seeded questions (seed 0 rejected) with two
  wordings each and unbiased random numbers; a list to sort is never shown
  already sorted. Verdicts ≥9 pass, ≤7 fail, 8 uncertain; fingerprint ≥8
  pass, ≤5 fail, no fresh baseline ⇒ uncertain. These thresholds and
  `τ_JS` are provisional until the observation period. Rate limits,
  timeouts, other HTTP errors and unparseable replies count as
  unanswered, never wrong: a verdict is given only when it holds however
  the unanswered questions would have gone. Each class run counts its
  unanswered questions by reason (429, timeout, 5xx, unparseable, other
  HTTP error, missing) and `ClassRun::evidence_summary()` states them.
  Fingerprint: Jensen–Shannon divergence in bits against the pooled
  two-run official baseline; `τ_JS` is the nearest-rank 95% quantile of
  the 10 per-question divergences between the two baseline runs (with 10
  questions, the largest); a baseline older than 30 days,
  or kept for another model version, protocol or sampling parameters,
  counts as no baseline. Capability: tool call (`record_number`, exact arguments),
  strict JSON (the reply must be the JSON object itself, a code fence
  fails), thinking-signature round trip on the Anthropic protocol only
  (HTTP 400 is a rejection and a hard failure), monthly long-context
  recall; only claimed items are checked, none claimed ⇒ not applicable.
  Only the capability class can be not applicable: probe results marked
  not applicable for the deterministic or fingerprint class are rejected.
  Hard failures: returned model of another family, usage field names
  exclusive to another vendor (the model's and the protocol's own names
  are allowed), thinking signature rejected where that item applies
  (Anthropic protocol, thinking claimed: `run_hard_failures` takes the
  route's claims). Model names are normalised (trimmed and lower-cased,
  then the alias table, whose mapped value is lower-cased and trimmed
  too, then the trailing date dropped) into family and vendor; default
  official-host table.
  Grades per arm and window by Beijing run day, first match wins:
  official direct (not graded; hard failures only flag "官方路线异常"),
  C, A, A−, B. Gate: official, A and A− pass; C fails; B fails with
  probes on and passes with probes off ("未验证（探针已关闭）"). Until the
  observation period (positive control on the official route, negative
  control with a smaller model) is validated, grades are display-only:
  every graded arm passes the gate and is shown as undetermined.
  `AuthenticityAssessment::gate_input()` feeds the step 8 `E_d` interface
  unchanged. Log-integrity gate (§11.3′) with integer comparisons:
  parse rate ≥0.95, same-family version drift ≤0.05, model field missing
  above 0.2 only flagged; other-family requests are hard failures, not
  counted twice. Probe budget: a run is allowed only if this Beijing
  month's spend plus its estimate stays within the budget (10 yuan by
  default). Route overhead diagnostics (§11.4): median TTFT, total time
  and output speed against official samples within ±1 hour.
  Examples `probe_plan` (prints OpenAI Chat Completions request bodies for
  the 10 deterministic questions plus the claimed tool-call and strict-JSON
  items; thinking-signature and long-context requests are not generated
  yet) and `probe_judge` (judges a replay), and `tools/probe_live.py`, a
  standard-library runner for a local, opt-in run against a real relay
  (https required, plain http only for a local mock server; key from the
  environment or a git-ignored `.env`, never
  written: raw and escaped output is checked for it; no headers recorded).
  A replay line of kind `ok` must carry a 2xx status. `tools/test_probe_live.py`
  (standard-library unittest, run in CI) tests the runner against a mock
  server on 127.0.0.1: 429, timeout, 4xx/5xx and unparseable
  classification, unanswered never judged wrong, refusal to write an
  echoed key. CI never touches the network beyond localhost.
- Core, spec step 12: weight sensitivity data (§12), new module
  `sensitivity`. Winner map on the 171-point grid: `Win(w)` is the tie
  group of highest `V` among the compared arms, ties reported whole;
  withheld in the first window and when the reference has no anchor
  (tier III). Each grid point is flagged for `Ω_s` and `Ω_b`, which are
  inputs (validated: on the 0.05 grid, no duplicates; `Ω_s` must be
  non-empty; an empty `Ω_b` means a behaviour signal with no qualifying
  grid point and is accepted, `None` means no behaviour signal; the
  report says which).
  Weight-robust winner over `Ω_s`, or over `max|w − W_k| ≤ 0.10` when
  there is no self-report, with per-arm shares and "在你的权重范围内都是它"
  only for a sole winner at every point. Statistical robustness: 200-resample
  task bootstrap stratified by (arm, cell), so every `n_iu`, `ω_u` and the
  design grid stay fixed and the reference and fallback arms are
  resampled like the others; seeded xorshift64 (seed recorded, 0
  rejected), caller recomputes each resample; reports winner frequency
  at `W_k`, `F_off` frequency, ties and resamples with an anchor. Flip
  line between the top two arms (`aΔ_C + bΔ_T + gΔ_D = 0`) with one
  statement per fixed dimension, evaluated on the feasible segment; the
  printed threshold is rounded up so the sentence is a sufficient
  condition. When the top two are within the tie tolerance, the
  "都高于" sentence carries "（当前差距在并列容差内，按 id 排序）".
  Withheld as well when there is no arm to compare. Every
  winner group is checked to hold an `F_off` arm (§12 property 1) in all
  builds; an arm dominated by a hair may share a tie group with its
  dominator.
  Noise-floor robustness at `W_k` under `c0`, `t0`, `ε` × 0.5 and × 2.
  All counters use checked addition. Paired with the spec example (Y
  wins 93 grid points, X 78; Y wins 14 of 14 in the self-report region).
- README (English and Chinese): section 10, weight sensitivity (§12),
  and its roadmap row and constants; built on the step 11 README.
- Core, spec step 11: five-level self-report (§10.2). The 171-point weight
  grid in exact twentieths, the 96-question bank, five-level banding (u
  rounded to 1e-9, closed bands), core and wide regions with one level of
  slack, adaptive question choice, the stop rule (4–10 questions,
  `|Omega_s| <= 15`), the point estimate `w_s`, the contradiction flag, and
  displayed values for a question (positive on both sides, defect rate at
  most 1, otherwise skipped).
- Core: behaviour calibration (§10.3), suggestions only. Choice events
  with whole-event dropping, constrained maximum likelihood for
  `theta >= 0`, the no-signal gates (events < 20, one chosen arm, rank < 3,
  `lambda_hat < 0.5`, not identifiable), profile likelihood, `Omega_b`,
  and a seeded 200-resample bootstrap. Behaviour from the scored window or
  later is an error.
  Separable choice data (some direction d >= 0 explains every choice at
  least as well as each alternative) are reported as not identifiable
  before the fit; bootstrap resamples that are separable count as failed.
- Core: the §10.4 merge table. Behaviour never changes the weights in
  effect.
- Tests compare the bank, every (question, grid point) level, the 528
  cut-point pairs and all 171 honest sessions with the reference script.

### Fixed
- Core: caller-supplied `family_vendor` prefixes and vendor names, and
  `official_hosts` keys and vendor names, are trimmed and lower-cased on
  lookup. A table written as `Anthropic` or ` API.Anthropic.com ` no
  longer mis-fires foreign usage or misses official-direct. An entry
  that is empty after trimming is ignored (DECISIONS 13.6, 13.7).
  A host key that is not already canonical overrides the built-in row
  for that host, so a mixed-case insert is not dropped because of
  `BTreeMap` order. The same normalized family-prefix length keeps the
  later entry, so `push` overrides a built-in prefix.
- Docs: `BaselineKey` sampling is documented as trim-only. The canonical
  form is lower-case `key=value` parameters joined in a stable order
  (for example `temperature=1`). `Temperature=1` and `temperature=1`
  stay different keys, so the fingerprint class is uncertain rather than
  a false fail. Noted on `BaselineKey` / `BaselineKey::new` and in the
  English and Chinese READMEs. Matching behaviour is unchanged.

### Changed
- Core, authenticity `grade` (DECISIONS 13.13): the A− fingerprint condition
  is monotonic. Fingerprint qualifies unless it has 0 pass days and at
  least 1 fail day, so these three combinations are A− (the literal
  reading graded them B): deterministic passing on 1 day, capability
  passing on 1 day, and fingerprint passing on 2 or more days;
  deterministic passing on 1 day, capability passing on 2 or more days,
  and fingerprint passing on 2 or more days; deterministic passing on
  2 or more days, capability passing on 1 day, and fingerprint passing
  on 2 or more days. A single fingerprint fail day with no pass day is
  still B; two or more fingerprint fail days is C. Exactly one
  fingerprint pass day is still A− when the other A− guards hold (no
  hard-failure day, deterministic and capability each passing on at
  least 1 day, at most one capability fail day) and fingerprint fail
  days are fewer than 2. The English and Chinese READMEs state the same
  condition.
- Core, authenticity `grade` (DECISIONS 13.15): capability fail days cap
  both A and A−. With `cf` the number of capability fail days, A and A−
  each require their existing conditions and `cf ≤ 1`. If `cf ≥ 2` and
  the arm is not already C, the grade is at most B. Capability fails
  still do not count toward C.
- Core: `VendorTables::vendor` and `VendorTables::official_vendor` now
  return `Option<String>` instead of `Option<&str>`. The string is the
  trimmed, lower-cased vendor, which may not be the text stored in the
  public fields. `agent-value-core` is a workspace-internal crate
  (`publish = false` on the workspace package), not a published crate.
- README section 9 (English and Chinese): in a question's equal dimension
  both options show the base up to at most 1 ulp of (base + noise floor).
- Core: `resolve_fallback` validates the candidate (finite, non-negative
  layer weights; no duplicate rating cells) also when the user chose the
  reference arm itself; valid input still gives `Default`.
- Core, step 10 review follow-up: the diagnostic reference anchor takes
  the worst valid value per dimension, so a valid reference cell with one
  NA dimension still contributes its other dimensions (§7 line 570).
- Core: stricter input checks. All six `overall_arm` counters (`n_comp`,
  `u`, `n_c`, `n_sub`, `n_miss`, `n_fail`) use checked addition and return
  an overflow error instead of wrapping; fallback layer weights that are
  NaN, infinite or negative are an error (`fallback_failures` and
  `resolve_fallback` now return `Result`); a non-finite `κ_eff` is not
  admitted; duplicate cells in fallback ratings, duplicate arm ids in
  `layer_pick` scores and duplicate arm ids passed to `overall_top` are
  rejected.

### Added
- Core, spec step 10: reference anchor certification (§7). Tiers are
  decided III → II → I, first match wins; tier III lists every reason
  (miss, true-fail cell, NA cell, fewer than 3 valid cells), so a layer
  with fewer than 3 positive-weight cells is never certified. Tier II
  clamps the layer anchor, never cell values: cost and time
  `clip(A, (1−θ)a, (1+θ)a)`, quality through defects `D = 1 − Q`,
  `θ = 0.10`, with the clamp metadata kept. Layer `V` uses the anchor and
  is never normalized by the pure-route score. Unclamped diagnostic
  reference anchor per the §7 formula. Layer official #1 is the tie group
  of highest `V` within `F_rec` (tiers I/II only); with `F_rec` empty,
  `F_rob` is listed as "取消稳健备选" and nobody is #1. Overall closure
  (§6): the reference must be tier I or II in every positive-weight
  layer (overall anchor = `Σ ω_d` of layer anchors in all three
  dimensions, "含 Ⅱ 档夹逼层" header); an arm gets an overall `V` only when
  it passes the grid gates and is in `F_all`; overall triple, `cov_i` and
  the partial-coverage flag are reported. `robust_enabled` and the
  "账单过粗" tag are now set from cell notes; a tagged arm can be #1 and
  keeps its tag on every row. §13: `R_k` selection (user → official
  direct → most tasks, availability = authenticity passed and no
  `F_bad`, ties by arm id) and `F_k` qualification (authenticity, no
  `F_bad`, tier I/II, at least 3 quality ratings per cell; otherwise
  revert to `R_k` with a version note). Diagnostic rows report
  `not_admitted` reasons. Display strings live in `labels`. Paired with
  v22 examples 6/7/8 composed from `E_d` to the official pick.

- Core, spec step 9: diagnostic table for arms with miss/fail cells (§5) —
  ESS admission (`n_comp + κ_eff`, κ_eff = §1 prior κ averaged to the
  layer by `ω_u`, per v22 §0; a `d+` cell without a prior counts 0),
  miss fill via weighted lower median (DECISIONS E) and worst-valid per
  dimension (DECISIONS F), fail fill as the worse of measured and
  worst-valid; sort by coverage then model name. Layer frontiers
  `F_off` / `F_rob` / `F_rec` (§6); arms with an NA triple are skipped,
  and an NA robust triple also keeps an arm out of `F_rec`;
  lower-bound-only cancel-bill arms stay in `F_rec` and are listed in
  `coarse_bill` for the "账单过粗" tag (§2).
  Eligibility primary attribution maps seven labels to five §5 gates and
  suppresses spurious rate gates when complete-case already failed
  (DECISIONS P). Paired with v22 examples 6/7/8 (F_thin / F_bad paths;
  diagnostic anchor 0.7875).
- Core, spec step 8: layer eligibility set `E_d` (§5). Five gates —
  complete-case (`N^miss=N^fail=0`), sample (`n^comp >= max(30, 3 U_d)`),
  cancel and completion rates via integer comparisons
  (`5 n^C <= N^sub`, `2 n^comp >= N^sub`, with `N^sub > 0` first),
  authenticity / log-integrity (stub inputs; A/A−/B pass, B labelled
  undetermined, C fails), and reference-base (every `d+` cell has a
  complete `(C*,T*,Q*)` for R). `LayerStatus` now also exposes `n_c` and
  `N^sub`. Diagnostic-table rules and F_k eligibility belong to later
  steps.
- Core, spec steps 6-7: per-cell `C*`, `T*`, `Q*` (§2, §3): winsorized side
  means, shrinkage to the benchmark (`kappa_C = 5`, none without a
  benchmark), empty-sample chain benchmark -> layer pool -> NA, cancel
  amortization with the cancel-mean chain, fallback arm `F_k` (default
  `R_k`; reference and fallback are terminal), NA propagation, retry-wait
  term `E[n_W] D`, matched-failure quality `Qbar_F`; cancel-robust versions
  with per-task / capped / lower-bound excess and the monotone guards.
  Aggregation over positive-weight cells and layers (NA stays NA) and the
  value score `V` in log space with per-scope floors (§4).
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
