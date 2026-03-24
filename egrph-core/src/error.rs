use std::fmt;

#[derive(Debug, Clone)]
pub enum CypherError {
    ParseError(String),
    TypeError(String),
    SemanticError(String),
    RuntimeError(String),
    NotImplemented(String),
}

impl fmt::Display for CypherError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CypherError::ParseError(msg) => write!(f, "Parse error: {}", msg),
            CypherError::TypeError(msg) => write!(f, "Type error: {}", msg),
            CypherError::SemanticError(msg) => write!(f, "Semantic error: {}", msg),
            CypherError::RuntimeError(msg) => write!(f, "Runtime error: {}", msg),
            CypherError::NotImplemented(msg) => write!(f, "Not implemented: {}", msg),
        }
    }
}

impl std::error::Error for CypherError {}
