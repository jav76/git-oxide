use thiserror::Error;

/// Domain errors returned by `git-oxide-engine` operations.
#[derive(Debug, Error)]
pub enum EngineError {
    #[error("failed to open git repository at '{path}': {message}")]
    RepositoryOpen { path: String, message: String },

    #[error("git operation failed: {0}")]
    GitOperation(String),

    #[error("oid cache failure: {0}")]
    CacheError(String),

    #[error("serialization failure: {0}")]
    SerializationError(String),

    #[error("syntax highlighting failure: {0}")]
    HighlightError(String),

    #[error("invalid git object id '{0}'")]
    InvalidObjectId(String),

    #[error("repository path '{0}' not found in commit tree")]
    PathNotFound(String),

    #[error("background operation cancelled")]
    Cancelled,
}
