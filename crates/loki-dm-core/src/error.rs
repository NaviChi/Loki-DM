use thiserror::Error;

#[derive(Debug, Error)]
pub enum LokiDmError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("toml decode error: {0}")]
    TomlDe(#[from] toml::de::Error),
    #[error("toml encode error: {0}")]
    TomlSer(#[from] toml::ser::Error),
    #[error("invalid url: {0}")]
    Url(#[from] url::ParseError),
    #[error("server returned status {0}")]
    HttpStatus(reqwest::StatusCode),
    #[error("remote server did not provide content length")]
    MissingContentLength,
    #[error("remote server does not support byte ranges")]
    NoRangeSupport,
    #[error("remote server ignored byte range request")]
    RangeNotHonored,
    #[error("download cancelled")]
    Cancelled,
    #[error("task join error: {0}")]
    Join(#[from] tokio::task::JoinError),
    #[error("operation timed out")]
    Timeout,
    #[error("{0}")]
    Message(String),
}

pub type Result<T> = std::result::Result<T, LokiDmError>;
