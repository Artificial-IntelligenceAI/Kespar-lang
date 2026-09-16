//! Lexical structure (design/language.md §1).

use crate::CompileError;

#[derive(Clone, Debug, PartialEq)]
pub enum Tok {
    Int(i128),
    /// The literal's source text (`1.5`, `2.0e10`); parsed at the width its context gives it.
    Bin(String),
    Text(String),
    /// A quoted name, without the quotes.
    Name(String),
    /// A bare word: keyword, function name, chain segment or operator word.
    Ident(String),
    /// The `\n` newline piece.
    NewlinePiece,
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
    Assign,       // =
    Walrus,       // :=
    DollarAssign, // $=
    DollarWalrus, // $:=
    Eq,           // ==
    Neq,          // !==
    Lt,
    Gt,
    Le, // <==
    Ge, // >==
    Plus,
    Minus,
    Slash,
    Eof,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub tok: Tok,
    pub line: usize,
    pub col: usize,
}

pub fn lex(src: &str) -> Result<Vec<Token>, CompileError> {
    let chars: Vec<char> = src.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    let mut line = 1;
    let mut col = 1;
    let n = chars.len();

    macro_rules! push {
        ($t:expr, $l:expr, $c:expr) => {
            toks.push(Token { tok: $t, line: $l, col: $c })
        };
    }

    while i < n {
        let c = chars[i];
        let (tl, tc) = (line, col);
        // whitespace
        if c == '\n' {
            i += 1;
            line += 1;
            col = 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            col += 1;
            continue;
        }
        // comment
        if c == '/' && i + 1 < n && chars[i + 1] == '/' {
            while i < n && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        // integer or bin literal
        if c.is_ascii_digit() {
            let start = i;
            let mut digits = String::new();
            let mut prev_underscore = false;
            while i < n && (chars[i].is_ascii_digit() || chars[i] == '_') {
                if chars[i] == '_' {
                    if prev_underscore || i + 1 >= n || !chars[i + 1].is_ascii_digit() {
                        return Err(CompileError::new(
                            "'_' in a number must sit between two digits",
                            tl,
                        ));
                    }
                    prev_underscore = true;
                } else {
                    digits.push(chars[i]);
                    prev_underscore = false;
                }
                i += 1;
            }
            // bin literal: digits '.' digits [e[+-]digits]
            if i < n && chars[i] == '.' {
                if i + 1 < n && chars[i + 1].is_ascii_digit() {
                    if digits.contains('_') {
                        return Err(CompileError::new("'_' is not allowed in a bin literal", tl));
                    }
                    let mut text = digits.clone();
                    text.push('.');
                    i += 1;
                    while i < n && chars[i].is_ascii_digit() {
                        text.push(chars[i]);
                        i += 1;
                    }
                    if i < n && (chars[i] == 'e' || chars[i] == 'E') {
                        let mut j = i + 1;
                        let mut exp = String::from("e");
                        if j < n && (chars[j] == '+' || chars[j] == '-') {
                            exp.push(chars[j]);
                            j += 1;
                        }
                        if j < n && chars[j].is_ascii_digit() {
                            while j < n && chars[j].is_ascii_digit() {
                                exp.push(chars[j]);
                                j += 1;
                            }
                            text.push_str(&exp);
                            i = j;
                        } else {
                            return Err(CompileError::new(
                                "a bin exponent needs digits (write 2.0e10)",
                                tl,
                            ));
                        }
                    }
                    if i < n && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == '.') {
                        return Err(CompileError::new("malformed bin literal", tl));
                    }
                    col += i - start;
                    push!(Tok::Bin(text), tl, tc);
                    continue;
                } else {
                    return Err(CompileError::new(
                        "a bin literal needs digits on both sides of '.' (write 1.0, not 1.)",
                        tl,
                    ));
                }
            }
            if i < n && (chars[i].is_ascii_alphabetic() || chars[i] == '_') {
                return Err(CompileError::new("malformed integer literal", tl));
            }
            let v: i128 = digits
                .parse()
                .map_err(|_| CompileError::new("integer literal too large", tl))?;
            col += i - start;
            push!(Tok::Int(v), tl, tc);
            continue;
        }
        if c == '.' && i + 1 < n && chars[i + 1].is_ascii_digit() {
            // could be `.5` — only if not part of a chain like `var.int32`; a chain segment never
            // starts with a digit, so this is always the refused `.5` form.
            return Err(CompileError::new(
                "a bin literal needs digits on both sides of '.' (write 0.5, not .5)",
                tl,
            ));
        }
        // bare word
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < n && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            let w: String = chars[start..i].iter().collect();
            col += i - start;
            push!(Tok::Ident(w), tl, tc);
            continue;
        }
        // quoted name
        if c == '\'' {
            let mut j = i + 1;
            let mut name = String::new();
            loop {
                if j >= n || chars[j] == '\n' {
                    return Err(CompileError::new("unterminated quoted name", tl));
                }
                if chars[j] == '\'' {
                    break;
                }
                name.push(chars[j]);
                j += 1;
            }
            if name.is_empty() {
                return Err(CompileError::new("a quoted name cannot be empty", tl));
            }
            col += j + 1 - i;
            i = j + 1;
            push!(Tok::Name(name), tl, tc);
            continue;
        }
        // text literal
        if c == '"' {
            let mut j = i + 1;
            let mut text = String::new();
            loop {
                if j >= n || chars[j] == '\n' {
                    return Err(CompileError::new(
                        "unterminated text literal (a raw newline may not appear inside \"…\")",
                        tl,
                    ));
                }
                if chars[j] == '"' {
                    break;
                }
                if chars[j] == '\\' {
                    if j + 1 < n && (chars[j + 1] == '\\' || chars[j + 1] == '"') {
                        text.push(chars[j + 1]);
                        j += 2;
                        continue;
                    }
                    return Err(CompileError::new(
                        "only \\\\ and \\\" may be escaped inside text; a newline is the \\n piece outside the quotes",
                        tl,
                    ));
                }
                text.push(chars[j]);
                j += 1;
            }
            col += j + 1 - i;
            i = j + 1;
            push!(Tok::Text(text), tl, tc);
            continue;
        }
        // newline piece
        if c == '\\' {
            if i + 1 < n && chars[i + 1] == 'n' {
                i += 2;
                col += 2;
                push!(Tok::NewlinePiece, tl, tc);
                continue;
            }
            return Err(CompileError::new("unexpected '\\' (the newline piece is \\n)", tl));
        }
        // punctuation and operators
        let two: String = chars[i..(i + 2).min(n)].iter().collect();
        let three: String = chars[i..(i + 3).min(n)].iter().collect();
        let (tok, len) = match c {
            '[' => (Tok::LBrack, 1),
            ']' => (Tok::RBrack, 1),
            '{' => (Tok::LBrace, 1),
            '}' => (Tok::RBrace, 1),
            '(' => (Tok::LParen, 1),
            ')' => (Tok::RParen, 1),
            ',' => (Tok::Comma, 1),
            ';' => (Tok::Semi, 1),
            '.' => (Tok::Dot, 1),
            ':' if two == "::" => (Tok::ColonColon, 2),
            ':' if two == ":=" => (Tok::Walrus, 2),
            '$' if three == "$:=" => (Tok::DollarWalrus, 3),
            '$' if two == "$=" => (Tok::DollarAssign, 2),
            '=' if two == "==" => (Tok::Eq, 2),
            '=' => (Tok::Assign, 1),
            '!' if three == "!==" => (Tok::Neq, 3),
            '!' if two == "!=" => {
                return Err(CompileError::new("'!=' is not an operator; write '!=='", tl))
            }
            '<' if three == "<==" => (Tok::Le, 3),
            '<' if two == "<=" => {
                return Err(CompileError::new("'<=' is not an operator; write '<=='", tl))
            }
            '<' => (Tok::Lt, 1),
            '>' if three == ">==" => (Tok::Ge, 3),
            '>' if two == ">=" => {
                return Err(CompileError::new("'>=' is not an operator; write '>=='", tl))
            }
            '>' => (Tok::Gt, 1),
            '+' if two == "++" => {
                return Err(CompileError::new(
                    "'++' is not an operator; write 'n' = ['n' + 1]",
                    tl,
                ))
            }
            '+' if two == "+=" => {
                return Err(CompileError::new(
                    "'+=' is not an operator; write 'n' = ['n' + 1]",
                    tl,
                ))
            }
            '+' => (Tok::Plus, 1),
            '-' => (Tok::Minus, 1),
            '/' => (Tok::Slash, 1),
            '*' => return Err(CompileError::new("'*' is not an operator; write 'x'", tl)),
            '^' => return Err(CompileError::new("'^' is not an operator; write 'xx'", tl)),
            '%' => return Err(CompileError::new("'%' is not an operator; write 'mod'", tl)),
            _ => {
                return Err(CompileError::new(
                    &format!("unexpected character '{}'", c),
                    tl,
                ))
            }
        };
        i += len;
        col += len;
        push!(tok, tl, tc);
    }
    push!(Tok::Eof, line, col);
    Ok(toks)
}
