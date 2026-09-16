//! Tokens for `.kpls` source. See design/language.md §1.

use crate::diag::CompileError;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Int(String),  // digits with `_` removed
    Bin(String),  // as written, `_` removed
    Text(String), // "..." with escapes resolved
    Name(String), // '...'
    Ident(String),
    Newline,      // the `\n` piece
    LBrack,
    RBrack,
    LBrace,
    RBrace,
    LParen,
    RParen,
    Comma,
    Semi,
    Dot,
    ColonColon,
    Walrus,       // :=
    DollarWalrus, // $:=
    DollarEq,     // $=
    Eq,           // =
    EqEq,         // ==
    NotEq,        // !==
    Lt,
    Gt,
    Le,           // <==
    Ge,           // >==
    Plus,
    Minus,
    Slash,
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub line: u32,
    /// Whitespace (or a comment) came right before this token. Inside `[ ]`
    /// that is the difference between `'xs'[0]` (an index) and `'xs' [0]` (two pieces).
    pub space_before: bool,
}

pub fn lex(src: &str) -> Result<Vec<Token>, CompileError> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line: u32 = 1;
    let mut space = true;
    let push = |out: &mut Vec<Token>, tok: Tok, line: u32, space: &mut bool| {
        out.push(Token { tok, line, space_before: *space });
        *space = false;
    };
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\n' => {
                line += 1;
                i += 1;
                space = true;
            }
            ' ' | '\t' | '\r' => {
                i += 1;
                space = true;
            }
            '/' if i + 1 < chars.len() && chars[i + 1] == '/' => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                space = true;
            }
            '[' => { push(&mut out, Tok::LBrack, line, &mut space); i += 1; }
            ']' => { push(&mut out, Tok::RBrack, line, &mut space); i += 1; }
            '{' => { push(&mut out, Tok::LBrace, line, &mut space); i += 1; }
            '}' => { push(&mut out, Tok::RBrace, line, &mut space); i += 1; }
            '(' => { push(&mut out, Tok::LParen, line, &mut space); i += 1; }
            ')' => { push(&mut out, Tok::RParen, line, &mut space); i += 1; }
            ',' => { push(&mut out, Tok::Comma, line, &mut space); i += 1; }
            ';' => { push(&mut out, Tok::Semi, line, &mut space); i += 1; }
            '.' => {
                if i + 1 < chars.len() && chars[i + 1].is_ascii_digit() {
                    return Err(CompileError::new(line, "a bin literal needs digits before the `.` (write `0.5`)"));
                }
                push(&mut out, Tok::Dot, line, &mut space);
                i += 1;
            }
            '+' => {
                if i + 1 < chars.len() && (chars[i + 1] == '+' || chars[i + 1] == '=') {
                    return Err(CompileError::new(line, "there is no `++` or `+=`; write `'i' = ['i' + 1];`"));
                }
                push(&mut out, Tok::Plus, line, &mut space);
                i += 1;
            }
            '-' => {
                if i + 1 < chars.len() && (chars[i + 1] == '-' || chars[i + 1] == '=') {
                    return Err(CompileError::new(line, "there is no `--` or `-=`; write `'i' = ['i' - 1];`"));
                }
                push(&mut out, Tok::Minus, line, &mut space);
                i += 1;
            }
            '/' => { push(&mut out, Tok::Slash, line, &mut space); i += 1; }
            '*' => return Err(CompileError::new(line, "`*` is not an operator; multiply is `x`")),
            '^' => return Err(CompileError::new(line, "`^` is not an operator; power is `xx`")),
            '%' => return Err(CompileError::new(line, "`%` is not an operator; remainder is `mod`")),
            ':' => {
                if i + 1 < chars.len() && chars[i + 1] == ':' {
                    push(&mut out, Tok::ColonColon, line, &mut space);
                    i += 2;
                } else if i + 1 < chars.len() && chars[i + 1] == '=' {
                    push(&mut out, Tok::Walrus, line, &mut space);
                    i += 2;
                } else {
                    return Err(CompileError::new(line, "stray `:`"));
                }
            }
            '$' => {
                if chars.get(i + 1) == Some(&':') && chars.get(i + 2) == Some(&'=') {
                    push(&mut out, Tok::DollarWalrus, line, &mut space);
                    i += 3;
                } else if chars.get(i + 1) == Some(&'=') {
                    push(&mut out, Tok::DollarEq, line, &mut space);
                    i += 2;
                } else {
                    return Err(CompileError::new(line, "stray `$` (shadowing is `$:=` or `$=`)"));
                }
            }
            '=' => {
                if chars.get(i + 1) == Some(&'=') {
                    push(&mut out, Tok::EqEq, line, &mut space);
                    i += 2;
                } else {
                    push(&mut out, Tok::Eq, line, &mut space);
                    i += 1;
                }
            }
            '!' => {
                if chars.get(i + 1) == Some(&'=') && chars.get(i + 2) == Some(&'=') {
                    push(&mut out, Tok::NotEq, line, &mut space);
                    i += 3;
                } else if chars.get(i + 1) == Some(&'=') {
                    return Err(CompileError::new(line, "not-equal is `!==`, not `!=`"));
                } else {
                    return Err(CompileError::new(line, "stray `!` (boolean not is the word `not`)"));
                }
            }
            '<' => {
                if chars.get(i + 1) == Some(&'=') && chars.get(i + 2) == Some(&'=') {
                    push(&mut out, Tok::Le, line, &mut space);
                    i += 3;
                } else if chars.get(i + 1) == Some(&'=') {
                    return Err(CompileError::new(line, "less-or-equal is `<==`, not `<=`"));
                } else {
                    push(&mut out, Tok::Lt, line, &mut space);
                    i += 1;
                }
            }
            '>' => {
                if chars.get(i + 1) == Some(&'=') && chars.get(i + 2) == Some(&'=') {
                    push(&mut out, Tok::Ge, line, &mut space);
                    i += 3;
                } else if chars.get(i + 1) == Some(&'=') {
                    return Err(CompileError::new(line, "greater-or-equal is `>==`, not `>=`"));
                } else {
                    push(&mut out, Tok::Gt, line, &mut space);
                    i += 1;
                }
            }
            '\\' => {
                if chars.get(i + 1) == Some(&'n') {
                    push(&mut out, Tok::Newline, line, &mut space);
                    i += 2;
                } else {
                    return Err(CompileError::new(line, "stray `\\` (the newline piece is `\\n`)"));
                }
            }
            '"' => {
                i += 1;
                let mut s = String::new();
                loop {
                    match chars.get(i) {
                        None => return Err(CompileError::new(line, "text literal not closed")),
                        Some('"') => { i += 1; break; }
                        Some('\n') => return Err(CompileError::new(line, "a text literal may not contain a raw newline; use the `\\n` piece")),
                        Some('\\') => match chars.get(i + 1) {
                            Some('\\') => { s.push('\\'); i += 2; }
                            Some('"') => { s.push('"'); i += 2; }
                            Some('n') => return Err(CompileError::new(line, "`\\n` is a piece, not an escape: write `[\"line 1\" \\n \"line 2\"]`")),
                            _ => return Err(CompileError::new(line, "unknown escape in text (only `\\\\` and `\\\"`)")),
                        },
                        Some(&ch) => { s.push(ch); i += 1; }
                    }
                }
                push(&mut out, Tok::Text(s), line, &mut space);
            }
            '\'' => {
                i += 1;
                let mut s = String::new();
                loop {
                    match chars.get(i) {
                        None | Some('\n') => return Err(CompileError::new(line, "name not closed (names are quoted: `'total'`)")),
                        Some('\'') => { i += 1; break; }
                        Some(&ch) => { s.push(ch); i += 1; }
                    }
                }
                if s.is_empty() {
                    return Err(CompileError::new(line, "empty name"));
                }
                push(&mut out, Tok::Name(s), line, &mut space);
            }
            c if c.is_ascii_digit() => {
                let start = i;
                let mut s = String::new();
                let mut is_bin = false;
                while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '_') {
                    if chars[i] == '_' {
                        if !chars.get(i + 1).map_or(false, |c| c.is_ascii_digit()) || !chars[i - 1].is_ascii_digit() {
                            return Err(CompileError::new(line, "`_` may only sit between digits"));
                        }
                    } else {
                        s.push(chars[i]);
                    }
                    i += 1;
                }
                if i < chars.len() && chars[i] == '.' && chars.get(i + 1).map_or(false, |c| c.is_ascii_digit()) {
                    is_bin = true;
                    s.push('.');
                    i += 1;
                    while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '_') {
                        if chars[i] != '_' { s.push(chars[i]); }
                        i += 1;
                    }
                } else if i < chars.len() && chars[i] == '.' {
                    return Err(CompileError::new(line, "a bin literal needs digits after the `.` (write `1.0`)"));
                }
                if i < chars.len() && (chars[i] == 'e' || chars[i] == 'E') {
                    let mut j = i + 1;
                    if j < chars.len() && (chars[j] == '+' || chars[j] == '-') { j += 1; }
                    if j < chars.len() && chars[j].is_ascii_digit() {
                        is_bin = true;
                        s.push('e');
                        if chars[i + 1] == '-' { s.push('-'); }
                        i = j;
                        while i < chars.len() && chars[i].is_ascii_digit() {
                            s.push(chars[i]);
                            i += 1;
                        }
                    }
                }
                if i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    let _ = start;
                    return Err(CompileError::new(line, "a number runs straight into a word"));
                }
                push(&mut out, if is_bin { Tok::Bin(s) } else { Tok::Int(s) }, line, &mut space);
            }
            c if c.is_alphabetic() || c == '_' => {
                let mut s = String::new();
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    s.push(chars[i]);
                    i += 1;
                }
                push(&mut out, Tok::Ident(s), line, &mut space);
            }
            other => return Err(CompileError::new(line, format!("unexpected character `{other}`"))),
        }
    }
    push(&mut out, Tok::Eof, line, &mut space);
    Ok(out)
}
