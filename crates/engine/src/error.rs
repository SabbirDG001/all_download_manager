use std::io;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("file error: {0}")]
    Io(#[from] io::Error),
    #[error("server answered with status {0}")]
    BadStatus(reqwest::StatusCode),
    #[error("server ignored the range request (sent the whole file instead of a part)")]
    RangeNotHonored,
    #[error("connection closed before the segment finished")]
    ConnectionClosed,
    #[error("invalid header: {0}")]
    InvalidHeader(String),
    #[error("download was cancelled")]
    Cancelled,
    #[error("download finished with {got} of {expected} bytes")]
    Incomplete { got: u64, expected: u64 },
    #[error("invalid settings: {0}")]
    Settings(String),
}

impl Error {
    /// Whether trying the same request again might work.
    pub fn is_retryable(&self) -> bool {
        match self {
            Error::Http(_) | Error::ConnectionClosed => true,
            Error::BadStatus(s) => s.is_server_error() || s.as_u16() == 429,
            _ => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
