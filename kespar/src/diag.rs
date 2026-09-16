//! Compile errors. Printed as `error: <message> at line N` (language.md §9).

#[derive(Debug, Clone)]
pub struct CompileError {
    pub line: u32,
    pub message: String,
}

impl CompileError {
    pub fn new(line: u32, message: impl Into<String>) -> Self {
        CompileError { line, message: message.into() }
    }
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error: {} at line {}", self.message, self.line)
    }
}

pub type Result<T> = std::result::Result<T, CompileError>;
