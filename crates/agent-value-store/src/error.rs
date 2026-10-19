//! Store error type.

use std::fmt;

use agent_value_core::CoreError;

/// Result alias for the store.
pub type StoreResult<T> = std::result::Result<T, StoreError>;

/// Errors from the store. Bad input never panics.
#[derive(Debug)]
pub enum StoreError {
    /// A record failed core validation.
    Core(CoreError),
    /// SQLite reported an error (including CHECK, UNIQUE and trigger failures).
    Sqlite(rusqlite::Error),
    /// The write would change data that is immutable or already stored differently.
    Conflict(&'static str),
    /// Stored data could not be turned back into core types.
    Corrupt(String),
    /// The database was written by a newer version of this program.
    SchemaTooNew { found: i64, supported: i64 },
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Core(e) => write!(f, "{e}"),
            Self::Sqlite(e) => write!(f, "sqlite: {e}"),
            Self::Conflict(m) => write!(f, "conflict: {m}"),
            Self::Corrupt(m) => write!(f, "corrupt data: {m}"),
            Self::SchemaTooNew { found, supported } => write!(
                f,
                "database schema version {found} is newer than supported version {supported}"
            ),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<CoreError> for StoreError {
    fn from(e: CoreError) -> Self {
        Self::Core(e)
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}
