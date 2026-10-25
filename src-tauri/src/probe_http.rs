//! In-process probe HTTP sender.
//!
//! The probe panel command does not call this module. `run_probe_once`
//! and a due in-app schedule tick call `probe_run`, which sends through
//! the same crate on a blocking thread. `sends_http` is true because
//! those paths can send.
//! `tools/probe_live.py` remains the command-line runner.
//!
//! [`send`] and [`run`] block the calling thread. Call them off the Tauri
//! async runtime. Redirects are not followed, unlike the Python runner's
//! urllib client, so a 3xx reply is `http_error` and can differ from
//! `tools/probe_live.py`.
//!
//! Load the bearer token by variable name. Do not log that value or request
//! headers. [`SendResult`]'s `Debug` redacts reply text and the other
//! response fields that can echo the token.
//!
//! [`send`]: agent_value_probe_http::send
//! [`run`]: agent_value_probe_http::run
//! [`SendResult`]: agent_value_probe_http::SendResult

#![allow(unused_imports)]

pub use agent_value_probe_http::{
    base_url_allowed, load_key, load_key_from, run, send, PlanItem, ProbeHttpError, RunOptions,
    SendResult, REPLAY_HEADER,
};
