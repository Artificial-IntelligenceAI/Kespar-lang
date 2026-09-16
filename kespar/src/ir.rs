//! The checked, resolved program every later phase works on: names are
//! slots, functions are indices, every check site has a number, every
//! expression has a type (`Types::norm` it).

use crate::ast::{BinOp, Tier};
use crate::types::{Ty, Types};

pub type VarId = u32;
pub type FuncId = u32;
pub type SiteId = u32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SiteKind {
    Overflow,
    DivZero,
    OutOfBounds,
    NegExp,
}

impl SiteKind {
    pub fn text(self) -> &'static str {
        match self {
            SiteKind::Overflow => "overflow",
            SiteKind::DivZero => "division by zero",
            SiteKind::OutOfBounds => "out of bounds",
            SiteKind::NegExp => "negative exponent",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Site {
    pub id: SiteId,
    pub line: u32,
    pub kind: SiteKind,
    pub tier: Tier,
    pub func: FuncId,
    /// A short rendering of the operation, for the report.
    pub what: String,
    /// False once the finalize pass finds the operation is on bins (no check).
    pub active: bool,
}

#[derive(Debug, Clone)]
pub struct Expr {
    /// Unique per program, assigned by the checker's finalize pass; keys Precog's tables.
    pub id: u32,
    pub line: u32,
    pub ty: Ty,
    pub kind: EK,
}

#[derive(Debug, Clone)]
pub enum PieceIr {
    Text(String),
    Var(VarId, Ty),
    Newline,
    Value(Expr),
}

#[derive(Debug, Clone)]
pub struct ReadBounds {
    /// Scalar integer or list element bound.
    pub value: Option<(i128, i128)>,
    /// Length bound for str / list.
    pub len: Option<(i128, i128)>,
}

#[derive(Debug, Clone)]
pub enum EK {
    Int(i128),
    Bin(f64),
    Text(String),
    Bool(bool),
    Var(VarId),
    Pieces(Vec<PieceIr>),
    List(Vec<Expr>),
    /// Unary minus; the site is Overflow (integers only).
    Neg(Box<Expr>, Option<SiteId>),
    Not(Box<Expr>),
    /// `sites`: overflow site (if any), then the zero-divisor / negative-exponent site (if any).
    Binary(BinOp, Box<Expr>, Box<Expr>, Option<SiteId>, Option<SiteId>),
    Index(Box<Expr>, Box<Expr>, SiteId),
    Call(FuncId, Vec<Expr>),
    /// `std::read.stdin[...]`; `read` indexes `Program::reads`.
    Read(u32, ReadBounds),
    Len(Box<Expr>),
    Fill(Box<Expr>, Box<Expr>, SiteId),
    /// Conversion to `ty`; the site is Overflow when converting into an integer.
    To(Box<Expr>, Option<SiteId>),
}

#[derive(Debug, Clone)]
pub struct Stmt {
    pub line: u32,
    pub kind: SK,
}

#[derive(Debug, Clone)]
pub enum SK {
    Let(VarId, Expr),
    Assign(VarId, Expr),
    /// `target[index] = [value]`; `target` is a list-typed expression (a name or a nested index).
    AssignIndex(Expr, Expr, Expr, SiteId),
    Print { useful: bool, pieces: Vec<Expr> },
    Exit(Expr),
    If(Vec<(Expr, Vec<Stmt>)>, Option<Vec<Stmt>>),
    /// A `check { }` / `nocheck { }` block, or any plain block: a scope, nothing more.
    Block(Vec<Stmt>),
    Loop(Vec<Stmt>),
    While(Expr, Vec<Stmt>),
    ForRange { var: VarId, a: Expr, b: Expr, body: Vec<Stmt> },
    ForList { var: VarId, list: Expr, body: Vec<Stmt> },
    Break,
    Continue,
    Return(Option<Expr>),
    CallStmt(Expr),
}

#[derive(Debug, Clone)]
pub struct Local {
    pub name: String,
    pub ty: Ty,
    pub line: u32,
    pub immut: bool,
    /// Declared with `:=` (or an untyped parameter / loop variable / return).
    pub inferred: bool,
}

#[derive(Debug, Clone)]
pub struct Func {
    pub name: String,
    pub line: u32,
    pub params: Vec<VarId>,
    pub ret: Ty,
    pub locals: Vec<Local>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub struct ReadInfo {
    pub line: u32,
    pub name: String,
    pub ty: Ty,
    pub func: FuncId,
}

#[derive(Debug, Clone)]
pub struct Program {
    pub funcs: Vec<Func>,
    pub main: FuncId,
    pub sites: Vec<Site>,
    pub reads: Vec<ReadInfo>,
    pub types: Types,
    /// Every integer literal with its (already normalised) type, for the fit check.
    pub free_names: Vec<(String, u32)>,
}

impl Program {
    pub fn ty(&mut self, t: &Ty) -> Ty {
        self.types.norm(t)
    }
    pub fn has_reads(&self) -> bool {
        !self.reads.is_empty()
    }
}
