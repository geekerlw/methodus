pub mod catalog;
pub mod evolution;
pub mod hypothesis;
pub mod learning;
pub mod migration;
pub mod store;
pub mod usage;

pub use store::{EventRecord, Store};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("Migration error: {0}")]
    Migration(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
