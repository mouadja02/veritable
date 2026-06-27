use thiserror::Error;

#[derive(Debug, Error)]
pub enum VeritableError {
    #[error("connection error: {0}")]
    Connection(String),
    #[error("query error: {0}")]
    Query(String),
    #[error("schema error: {0}")]
    Schema(String),
}

pub type Result<T> = std::result::Result<T, VeritableError>;
