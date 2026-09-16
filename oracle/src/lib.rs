//! The Kespar reference interpreter, written from `design/language.md` alone.
//!
//! Deliberately slow, small and obvious: a tree-walking interpreter with a
//! static checker that performs §8.3 step-1 forcing and checks every check
//! site (§8.4) at run time, every time.

pub mod ast;
pub mod check;
pub mod interp;
pub mod lexer;
pub mod parser;
pub mod render;

use std::fmt;

/// A compile error (§9: exit 3, `error: … at line N`).
#[derive(Clone, Debug)]
pub struct CompileError {
    pub msg: String,
    pub line: usize,
}

impl CompileError {
    pub fn new(msg: &str, line: usize) -> CompileError {
        CompileError { msg: msg.to_string(), line }
    }
}

impl fmt::Display for CompileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "error: {} at line {}", self.msg, self.line)
    }
}

/// Options for one run.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Step budget; `None` means unlimited.
    pub steps: Option<u64>,
    pub trace_checks: bool,
}

/// The outcome of a run: everything the CLI prints.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    pub stdout: Vec<u8>,
    /// The single stderr line, without its newline, if any.
    pub stderr: Option<String>,
    pub exit: i32,
    /// `--trace-checks` lines (already ordered), printed to stdout after the run.
    pub trace: Vec<String>,
}

/// Lex, parse and check a program.
pub fn compile(src: &str) -> Result<check::Checked, CompileError> {
    let toks = lexer::lex(src)?;
    let program = parser::parse(toks)?;
    check::check(program)
}

/// The `--types` listing: one line per `:=` name.
pub fn types_listing(src: &str) -> Result<Vec<String>, CompileError> {
    let checked = compile(src)?;
    Ok(checked.types_listing())
}

/// Run a program on the given input.
pub fn run(src: &str, input: &[u8], opts: &Options) -> Outcome {
    let checked = match compile(src) {
        Ok(c) => c,
        Err(e) => {
            return Outcome {
                stdout: Vec::new(),
                stderr: Some(e.to_string()),
                exit: 3,
                trace: Vec::new(),
            }
        }
    };
    interp::run(&checked, input, opts)
}
