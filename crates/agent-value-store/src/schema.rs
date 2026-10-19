//! SQLite schema. Forward-only migrations tracked with `PRAGMA user_version`.

/// The reserved probe cell id used in the SQL CHECKs; a unit test asserts it
/// equals `agent_value_core::cells::PROBE_CELL_ID`.
#[cfg(test)]
pub(crate) const PROBE_CELL_SQL: &str = "__probe__";

/// Migration `i` brings the schema from version `i` to `i + 1`.
pub(crate) const MIGRATIONS: &[&str] = &[MIGRATION_1];

const MIGRATION_1: &str = r#"
CREATE TABLE arms (
    id            INTEGER PRIMARY KEY,
    model         TEXT NOT NULL,
    provider_kind TEXT NOT NULL CHECK (provider_kind IN ('official', 'relay', 'subscription')),
    host          TEXT NOT NULL,
    protocol      TEXT NOT NULL,
    client        TEXT NOT NULL,
    UNIQUE (model, provider_kind, host, protocol, client)
);

CREATE TABLE tasks (
    id                 TEXT PRIMARY KEY,
    scope              TEXT NOT NULL,
    cell               TEXT NOT NULL,
    is_probe           INTEGER NOT NULL CHECK (is_probe IN (0, 1)),
    arm_id             INTEGER NOT NULL REFERENCES arms(id),
    submitted_ms       INTEGER NOT NULL,
    ended_ms           INTEGER,
    outcome            TEXT NOT NULL DEFAULT 'pending'
                       CHECK (outcome IN ('pending', 'success', 'failure', 'cancel', 'undetermined')),
    first_msg_category TEXT,
    first_msg_chars    INTEGER,
    first_msg_digest   TEXT,
    active_ms          INTEGER CHECK (active_ms IS NULL OR active_ms >= 0),
    wait_ms            INTEGER CHECK (wait_ms IS NULL OR wait_ms >= 0),
    interruptions      INTEGER NOT NULL DEFAULT 0 CHECK (interruptions >= 0),
    rating             INTEGER CHECK (rating IS NULL OR rating BETWEEN 1 AND 5),
    CHECK ((is_probe = 1 AND cell = '__probe__') OR (is_probe = 0 AND cell <> '__probe__')),
    CHECK (is_probe = 0 OR rating IS NULL),
    CHECK ((first_msg_category IS NULL) = (first_msg_digest IS NULL)),
    CHECK ((first_msg_category IS NULL) = (first_msg_chars IS NULL)),
    CHECK (first_msg_chars IS NULL OR first_msg_chars >= 0),
    CHECK (ended_ms IS NULL OR ended_ms >= submitted_ms)
);
CREATE INDEX tasks_scope_time ON tasks (scope, submitted_ms);
CREATE INDEX tasks_arm ON tasks (arm_id);

CREATE TABLE requests (
    id                 TEXT PRIMARY KEY,
    task_id            TEXT REFERENCES tasks(id) ON DELETE CASCADE,
    arm_id             INTEGER NOT NULL REFERENCES arms(id),
    auxiliary          INTEGER NOT NULL CHECK (auxiliary IN (0, 1)),
    returned_model     TEXT,
    response_id        TEXT,
    key_env_var        TEXT,
    start_ms           INTEGER NOT NULL,
    first_token_ms     INTEGER,
    end_ms             INTEGER NOT NULL,
    input_tokens       INTEGER NOT NULL CHECK (input_tokens >= 0),
    output_tokens      INTEGER NOT NULL CHECK (output_tokens >= 0),
    cache_read_tokens  INTEGER NOT NULL CHECK (cache_read_tokens >= 0),
    cache_write_tokens INTEGER NOT NULL CHECK (cache_write_tokens >= 0),
    charged_micro_yuan INTEGER CHECK (charged_micro_yuan IS NULL OR charged_micro_yuan >= 0),
    CHECK (end_ms >= start_ms),
    CHECK (first_token_ms IS NULL OR (first_token_ms >= start_ms AND first_token_ms <= end_ms))
);
CREATE INDEX requests_start ON requests (start_ms);
CREATE INDEX requests_task ON requests (task_id);
CREATE INDEX requests_arm ON requests (arm_id);

CREATE TABLE behavior_events (
    id            TEXT PRIMARY KEY,
    at_ms         INTEGER NOT NULL,
    kind          TEXT NOT NULL
                  CHECK (kind IN ('manual-switch', 'reroute-after-failure', 'cancel', 'redo')),
    scope         TEXT NOT NULL,
    cell          TEXT NOT NULL CHECK (cell <> '__probe__'),
    chosen_arm_id INTEGER NOT NULL REFERENCES arms(id)
);
CREATE INDEX behavior_events_time ON behavior_events (at_ms);

CREATE TABLE event_available_arms (
    event_id TEXT NOT NULL REFERENCES behavior_events(id) ON DELETE CASCADE,
    arm_id   INTEGER NOT NULL REFERENCES arms(id),
    PRIMARY KEY (event_id, arm_id)
);

CREATE TABLE probe_results (
    id               INTEGER PRIMARY KEY,
    probe_id         TEXT NOT NULL,
    probe_version    TEXT NOT NULL,
    arm_id           INTEGER NOT NULL REFERENCES arms(id),
    run_day          TEXT NOT NULL,
    at_ms            INTEGER NOT NULL,
    class            TEXT NOT NULL CHECK (class IN ('deterministic', 'fingerprint', 'capability')),
    verdict          TEXT NOT NULL CHECK (verdict IN ('pass', 'fail', 'uncertain', 'not-applicable')),
    hard_failure     INTEGER NOT NULL CHECK (hard_failure IN (0, 1)),
    evidence_summary TEXT NOT NULL,
    UNIQUE (probe_id, probe_version, arm_id, run_day)
);
CREATE INDEX probe_results_day ON probe_results (run_day);
CREATE INDEX probe_results_time ON probe_results (at_ms);

CREATE TABLE cell_weights (
    scope  TEXT NOT NULL,
    window_index INTEGER NOT NULL CHECK (window_index >= 0),
    cell   TEXT NOT NULL,
    omega  REAL NOT NULL CHECK (omega >= 0 AND omega <= 1),
    PRIMARY KEY (scope, window_index, cell),
    CHECK (cell <> '__probe__' OR omega = 0)
);

CREATE TABLE weight_versions (
    scope          TEXT NOT NULL,
    id             INTEGER NOT NULL CHECK (id >= 0),
    a              REAL NOT NULL,
    b              REAL NOT NULL,
    g              REAL NOT NULL,
    source         TEXT NOT NULL
                   CHECK (source IN ('default', 'self-report', 'confirmed-behavior-suggestion', 'manual')),
    status         TEXT NOT NULL CHECK (status IN ('official', 'trial')),
    effective_from INTEGER,
    created_in     INTEGER,
    previous       INTEGER,
    PRIMARY KEY (scope, id),
    FOREIGN KEY (scope, previous) REFERENCES weight_versions (scope, id),
    CHECK (previous IS NULL OR previous < id),
    CHECK ((status = 'official' AND effective_from IS NOT NULL AND created_in IS NULL)
        OR (status = 'trial' AND effective_from IS NULL AND created_in IS NOT NULL))
);

CREATE TABLE weight_evidence (
    scope      TEXT NOT NULL,
    version_id INTEGER NOT NULL,
    seq        INTEGER NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('answer', 'event')),
    item       TEXT NOT NULL,
    level      INTEGER CHECK (level IS NULL OR level BETWEEN -2 AND 2),
    PRIMARY KEY (scope, version_id, seq),
    FOREIGN KEY (scope, version_id) REFERENCES weight_versions (scope, id),
    CHECK ((kind = 'answer') = (level IS NOT NULL))
);

CREATE TABLE trial_resolutions (
    scope        TEXT NOT NULL,
    version_id   INTEGER NOT NULL,
    resolution   TEXT NOT NULL CHECK (resolution IN ('confirmed', 'discarded')),
    confirmed_as INTEGER,
    PRIMARY KEY (scope, version_id),
    FOREIGN KEY (scope, version_id) REFERENCES weight_versions (scope, id),
    -- Deferred: a trial row and the later official version it became are
    -- written in the same transaction.
    FOREIGN KEY (scope, confirmed_as) REFERENCES weight_versions (scope, id)
        DEFERRABLE INITIALLY DEFERRED,
    CHECK ((resolution = 'confirmed') = (confirmed_as IS NOT NULL)),
    CHECK (confirmed_as IS NULL OR confirmed_as > version_id)
);

CREATE TABLE frozen_windows (
    scope                        TEXT NOT NULL,
    window_index                 INTEGER NOT NULL CHECK (window_index >= 0),
    weight_version_id            INTEGER NOT NULL,
    is_first_window              INTEGER NOT NULL CHECK (is_first_window IN (0, 1)),
    cell_weights_version         TEXT,
    reference_version            TEXT,
    fallback_version             TEXT,
    price_table_version          TEXT,
    classification_rules_version TEXT,
    PRIMARY KEY (scope, window_index),
    FOREIGN KEY (scope, weight_version_id) REFERENCES weight_versions (scope, id)
);

-- scope = '*' marks a version that applies to all scopes.
CREATE TABLE config_versions (
    kind           TEXT NOT NULL
                   CHECK (kind IN ('scope-config', 'classification-rules', 'price-table', 'reference', 'fallback')),
    scope          TEXT NOT NULL,
    label          TEXT NOT NULL,
    payload        TEXT NOT NULL,
    content_sha256 TEXT NOT NULL,
    created_ms     INTEGER NOT NULL,
    PRIMARY KEY (kind, scope, label)
);

-- Version and frozen-window rows are an audit trail: never updated in place.
CREATE TRIGGER cell_weights_immutable BEFORE UPDATE ON cell_weights
BEGIN SELECT RAISE(ABORT, 'cell_weights rows are immutable'); END;
CREATE TRIGGER weight_versions_immutable BEFORE UPDATE ON weight_versions
BEGIN SELECT RAISE(ABORT, 'weight_versions rows are immutable'); END;
CREATE TRIGGER weight_evidence_immutable BEFORE UPDATE ON weight_evidence
BEGIN SELECT RAISE(ABORT, 'weight_evidence rows are immutable'); END;
CREATE TRIGGER trial_resolutions_immutable BEFORE UPDATE ON trial_resolutions
BEGIN SELECT RAISE(ABORT, 'trial_resolutions rows are immutable'); END;
CREATE TRIGGER frozen_windows_immutable BEFORE UPDATE ON frozen_windows
BEGIN SELECT RAISE(ABORT, 'frozen_windows rows are immutable'); END;
CREATE TRIGGER config_versions_immutable BEFORE UPDATE ON config_versions
BEGIN SELECT RAISE(ABORT, 'config_versions rows are immutable'); END;
"#;

/// Tables in export order (parents before children).
pub(crate) const TABLES: &[&str] = &[
    "arms",
    "tasks",
    "requests",
    "behavior_events",
    "event_available_arms",
    "probe_results",
    "cell_weights",
    "weight_versions",
    "weight_evidence",
    "trial_resolutions",
    "frozen_windows",
    "config_versions",
];
