use std::fmt;
use std::io;

#[derive(Debug)]
pub enum OrbitError {
    Io(io::Error),
    Git(String),
    Parse(String),
    Network(String),
    Config(String),
    Mcp(String),
    Compile(String),
    Patch(String),
    InvalidInput(String),
}

impl fmt::Display for OrbitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "IO error: {}", e),
            Self::Git(msg) => write!(f, "Git error: {}", msg),
            Self::Parse(msg) => write!(f, "Parse error: {}", msg),
            Self::Network(msg) => write!(f, "Network error: {}", msg),
            Self::Config(msg) => write!(f, "Config error: {}", msg),
            Self::Mcp(msg) => write!(f, "MCP error: {}", msg),
            Self::Compile(msg) => write!(f, "Compile error: {}", msg),
            Self::Patch(msg) => write!(f, "Patch error: {}", msg),
            Self::InvalidInput(msg) => write!(f, "Invalid input: {}", msg),
        }
    }
}

impl std::error::Error for OrbitError {}

impl From<io::Error> for OrbitError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<serde_json::Error> for OrbitError {
    fn from(err: serde_json::Error) -> Self {
        Self::Parse(err.to_string())
    }
}

impl From<reqwest::Error> for OrbitError {
    fn from(err: reqwest::Error) -> Self {
        Self::Network(err.to_string())
    }
}

pub type Result<T> = std::result::Result<T, OrbitError>;
