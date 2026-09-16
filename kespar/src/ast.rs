//! The syntax tree, exactly as written. Types here are *spelled* types;
//! `check.rs` resolves them and infers the `:=` ones.

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeSpec {
    Int(IntWidth),
    Bin(BinWidth),
    Bool,
    Str,
    List(Box<TypeSpec>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum IntWidth {
    I8, I16, I32, I64, U8, U16, U32, U64,
}

impl IntWidth {
    pub fn name(self) -> &'static str {
        match self {
            IntWidth::I8 => "int8", IntWidth::I16 => "int16", IntWidth::I32 => "int32", IntWidth::I64 => "int64",
            IntWidth::U8 => "uint8", IntWidth::U16 => "uint16", IntWidth::U32 => "uint32", IntWidth::U64 => "uint64",
        }
    }
    pub fn min(self) -> i128 {
        match self {
            IntWidth::I8 => i8::MIN as i128, IntWidth::I16 => i16::MIN as i128,
            IntWidth::I32 => i32::MIN as i128, IntWidth::I64 => i64::MIN as i128,
            _ => 0,
        }
    }
    pub fn max(self) -> i128 {
        match self {
            IntWidth::I8 => i8::MAX as i128, IntWidth::I16 => i16::MAX as i128,
            IntWidth::I32 => i32::MAX as i128, IntWidth::I64 => i64::MAX as i128,
            IntWidth::U8 => u8::MAX as i128, IntWidth::U16 => u16::MAX as i128,
            IntWidth::U32 => u32::MAX as i128, IntWidth::U64 => u64::MAX as i128,
        }
    }
    pub fn signed(self) -> bool {
        matches!(self, IntWidth::I8 | IntWidth::I16 | IntWidth::I32 | IntWidth::I64)
    }
    pub fn bits(self) -> u32 {
        match self {
            IntWidth::I8 | IntWidth::U8 => 8, IntWidth::I16 | IntWidth::U16 => 16,
            IntWidth::I32 | IntWidth::U32 => 32, IntWidth::I64 | IntWidth::U64 => 64,
        }
    }
    pub fn holds(self, v: i128) -> bool {
        v >= self.min() && v <= self.max()
    }
    /// Two's-complement wrap of `v` into this width (for `nocheck` sites).
    pub fn wrap(self, v: i128) -> i128 {
        let bits = self.bits();
        let m = (1i128 << bits) - 1;
        let u = v & m;
        if self.signed() && u >= (1i128 << (bits - 1)) { u - (1i128 << bits) } else { u }
    }
    pub fn from_name(s: &str) -> Option<IntWidth> {
        Some(match s {
            "int8" => IntWidth::I8, "int16" => IntWidth::I16, "int32" => IntWidth::I32, "int64" => IntWidth::I64,
            "uint8" => IntWidth::U8, "uint16" => IntWidth::U16, "uint32" => IntWidth::U32, "uint64" => IntWidth::U64,
            _ => return None,
        })
    }
    pub const ALL: [IntWidth; 8] = [
        IntWidth::U8, IntWidth::I8, IntWidth::U16, IntWidth::I16, IntWidth::U32, IntWidth::I32, IntWidth::U64, IntWidth::I64,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinWidth { B32, B64 }

impl BinWidth {
    pub fn name(self) -> &'static str {
        match self { BinWidth::B32 => "bin32", BinWidth::B64 => "bin64" }
    }
}

impl TypeSpec {
    pub fn name(&self) -> String {
        match self {
            TypeSpec::Int(w) => w.name().to_string(),
            TypeSpec::Bin(w) => w.name().to_string(),
            TypeSpec::Bool => "bool".into(),
            TypeSpec::Str => "str.utf8".into(),
            TypeSpec::List(t) => format!("list.{}", t.name()),
        }
    }
}

pub type NodeId = u32;

#[derive(Debug, Clone)]
pub struct Expr {
    pub id: NodeId,
    pub line: u32,
    pub kind: ExprKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BinOp { Add, Sub, Mul, Pow, Div, Mod, Eq, Ne, Lt, Gt, Le, Ge, And, Or }

impl BinOp {
    pub fn spelling(self) -> &'static str {
        match self {
            BinOp::Add => "+", BinOp::Sub => "-", BinOp::Mul => "x", BinOp::Pow => "xx", BinOp::Div => "/",
            BinOp::Mod => "mod", BinOp::Eq => "==", BinOp::Ne => "!==", BinOp::Lt => "<", BinOp::Gt => ">",
            BinOp::Le => "<==", BinOp::Ge => ">==", BinOp::And => "and", BinOp::Or => "or",
        }
    }
    pub fn is_cmp(self) -> bool {
        matches!(self, BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UnOp { Neg, Not }

#[derive(Debug, Clone)]
pub enum Piece {
    Text(String),
    Name(String, u32),
    Newline,
    /// A bracketed value or an indexed name, rendered in place (provisional).
    Value(Expr),
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Int(i128),
    Bin(f64, String),
    Text(String),
    Bool(bool),
    Name(String),
    /// Whitespace-joined pieces: one text value.
    Pieces(Vec<Piece>),
    /// `[a, b, c]` — a list literal (two or more values, or `[v,]`).
    List(Vec<Expr>),
    Paren(Box<Expr>),
    /// `[v]` written inside another slot: the value itself, marked so it may be a piece.
    Bracket(Box<Expr>),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Index(Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    /// `std::read.stdin[bounds]`
    Read(Vec<Expr>),
    /// `std::range[a, b]`
    Range(Box<Expr>, Box<Expr>),
    Len(Box<Expr>),
    Fill(Box<Expr>, Box<Expr>),
    To(TypeSpec, Box<Expr>),
}

#[derive(Debug, Clone)]
pub struct Stmt {
    pub line: u32,
    pub kind: StmtKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mutability { Plain, Mut, Immut }

#[derive(Debug, Clone)]
pub enum StmtKind {
    /// `var[.mut|.immut][.T] 'name' (:=|=|$:=|$=) [v];`
    Var { mutability: Mutability, ty: Option<TypeSpec>, name: String, shadow: bool, value: Expr },
    Assign { name: String, value: Expr },
    AssignIndex { target: Expr, index: Expr, value: Expr },
    Print { useful: bool, pieces: Vec<Expr> },
    Exit(Expr),
    If { branches: Vec<(Expr, Vec<Stmt>)>, else_body: Option<Vec<Stmt>> },
    Loop(Vec<Stmt>),
    While(Expr, Vec<Stmt>),
    For { name: String, iter: Expr, body: Vec<Stmt> },
    Break,
    Continue,
    Return(Option<Expr>),
    Tier(Tier, Vec<Stmt>),
    CallStmt(Expr),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier { Default, Check, Nocheck }

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: Option<TypeSpec>,
    pub immut: bool,
    pub line: u32,
}

#[derive(Debug, Clone)]
pub struct Func {
    pub name: String,
    pub line: u32,
    pub ret: Option<TypeSpec>,
    pub params: Vec<Param>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub struct Program {
    pub funcs: Vec<Func>,
    pub main: Vec<Stmt>,
    pub main_line: u32,
}
