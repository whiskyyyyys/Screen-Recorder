use thiserror::Error;

pub type Result<T> = std::result::Result<T, ObsError>;

#[derive(Debug, Error)]
pub enum ObsError {
    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    #[error("Already exists: {0}")]
    AlreadyExists(String),

    #[error("Cycle detected in scene nesting: {0}")]
    SceneCycle(String),

    #[error("Capture error: {0}")]
    Capture(String),

    #[error("Audio error: {0}")]
    Audio(String),

    #[error("Renderer error: {0}")]
    Renderer(String),

    #[error("Encoder error: {0}")]
    Encoder(String),

    #[error("Recorder error: {0}")]
    Recorder(String),

    #[error("Streaming error: {0}")]
    Stream(String),

    #[error("Plugin error: {0}")]
    Plugin(String),

    #[error("WebSocket error: {0}")]
    WebSocket(String),

    #[error("Settings error: {0}")]
    Settings(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}
