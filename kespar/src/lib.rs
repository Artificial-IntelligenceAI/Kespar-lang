//! Kespar: a proof-of-concept of Cyborg's Precog. See design/*.md.

pub mod ast;
pub mod check;
pub mod diag;
pub mod emit;
pub mod ir;
pub mod lexer;
pub mod parser;
pub mod precog;
pub mod types;
pub mod vm;

use diag::Result;

/// Parse and check a source file into IR.
pub fn front(src: &str) -> Result<ir::Program> {
    let toks = lexer::lex(src)?;
    let ast = parser::parse(toks)?;
    check::check(&ast)
}
