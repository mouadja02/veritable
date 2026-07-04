use thiserror::Error;

#[derive(Debug, Error)]
pub enum VeritableError {
    #[error("Config error: {0}")]
    Config(String),
    #[error("Connectivity error: {0}")]
    Connectivity(String),
    #[error("Engine error: {0}")]
    Engine(String),
    #[error("Query error: {0}")]
    Query(String),
    #[error("Schema error: {0}")]
    Schema(String),
}

pub type Result<T> = std::result::Result<T, VeritableError>;

impl VeritableError {
    // u8 so it feeds `std::process::ExitCode::from` without a lossy cast.
    pub fn exit_code(&self) -> u8 {
        match self {
            VeritableError::Config(_) => 1,
            VeritableError::Connectivity(_) => 2,
            VeritableError::Engine(_) => 3,
            VeritableError::Query(_) => 4,
            VeritableError::Schema(_) => 5,
        }
    }
}
