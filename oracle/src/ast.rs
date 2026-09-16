//! Syntax tree for Kespar (design/language.md §1–§7).

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IntW {
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
}

impl IntW {
    pub fn name(self) -> &'static str {
        match self {
            IntW::I8 => "int8",
            IntW::I16 => "int16",
            IntW::I32 => "int32",
            IntW::I64 => "int64",
            IntW::U8 => "uint8",
            IntW::U16 => "uint16",
            IntW::U32 => "uint32",
            IntW::U64 => "uint64",
        }
    }
    pub fn from_name(s: &str) -> Option<IntW> {
        Some(match s {
            "int8" => IntW::I8,
            "int16" => IntW::I16,
            "int32" => IntW::I32,
            "int64" => IntW::I64,
            "uint8" => IntW::U8,
            "uint16" => IntW::U16,
            "uint32" => IntW::U32,
            "uint64" => IntW::U64,
            _ => return None,
        })
    }
    pub fn min(self) -> i128 {
        match self {
            IntW::I8 => i8::MIN as i128,
            IntW::I16 => i16::MIN as i128,
            IntW::I32 => i32::MIN as i128,
            IntW::I64 => i64::MIN as i128,
            _ => 0,
        }
    }
    pub fn max(self) -> i128 {
        match self {
            IntW::I8 => i8::MAX as i128,
            IntW::I16 => i16::MAX as i128,
            IntW::I32 => i32::MAX as i128,
            IntW::I64 => i64::MAX as i128,
            IntW::U8 => u8::MAX as i128,
            IntW::U16 => u16::MAX as i128,
            IntW::U32 => u32::MAX as i128,
            IntW::U64 => u64::MAX as i128,
        }
    }
    pub fn fits(self, v: i128) -> bool {
        v >= self.min() && v <= self.max()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BinW {
    B32,
    B64,
}

impl BinW {
    pub fn name(self) -> &'static str {
        match self {
            BinW::B32 => "bin32",
            BinW::B64 => "bin64",
        }
    }
}

/// An explicitly written type chain (`int32`, `str.utf8`, `list.int16`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeSpec {
    Int(IntW),
    Bin(BinW),
    Bool,
    Str,
    List(Box<TypeSpec>),
}

impl TypeSpec {
    pub fn render(&self) -> String {
        match self {
            TypeSpec::Int(w) => w.name().to_string(),
            TypeSpec::Bin(w) => w.name().to_string(),
            TypeSpec::Bool => "bool".to_string(),
            TypeSpec::Str => "str.utf8".to_string(),
            TypeSpec::List(t) => format!("list.{}", t.render()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Pow,
    Div,
    Mod,
    Eq,
    Neq,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
}

impl BinOp {
    pub fn spelling(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "x",
            BinOp::Pow => "xx",
            BinOp::Div => "/",
            BinOp::Mod => "mod",
            BinOp::Eq => "==",
            BinOp::Neq => "!==",
            BinOp::Lt => "<",
            BinOp::Gt => ">",
            BinOp::Le => "<==",
            BinOp::Ge => ">==",
            BinOp::And => "and",
            BinOp::Or => "or",
        }
    }
    pub fn is_cmp(self) -> bool {
        matches!(
            self,
            BinOp::Eq | BinOp::Neq | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge
        )
    }
    pub fn is_ordered_cmp(self) -> bool {
        matches!(self, BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge)
    }
    pub fn is_arith(self) -> bool {
        matches!(
            self,
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Pow | BinOp::Div | BinOp::Mod
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

#[derive(Clone, Debug)]
pub enum Piece {
    Newline,
    Expr(Expr),
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub id: usize,
    pub line: usize,
    pub col: usize,
    pub kind: ExprKind,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    IntLit(i128),
    BinLit(String),
    TextLit(String),
    BoolLit(bool),
    Name(String),
    Index(Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Unary(UnOp, Box<Expr>),
    /// `( … )` or a nested single-value `[ … ]`: grouping.
    Group(Box<Expr>),
    /// `[a, b, c]` — at least one element.
    ListLit(Vec<Expr>),
    /// Two or more whitespace-joined pieces, or a lone `\n`: a text.
    Pieces(Vec<Piece>),
    /// `std::read.stdin[bounds]`
    Read(Vec<Expr>),
    /// `std::range[a, b]`
    Range(Box<Expr>, Box<Expr>),
    /// `std::len[v]`
    Len(Box<Expr>),
    /// `std::fill[v, n]`
    Fill(Box<Expr>, Box<Expr>),
    /// `std::to.T[v]`
    To(TypeSpec, Box<Expr>),
}

#[derive(Clone, Debug)]
pub struct Block {
    pub line: usize,
    pub stmts: Vec<Stmt>,
}

#[derive(Clone, Debug)]
pub struct Stmt {
    pub line: usize,
    pub kind: StmtKind,
}

#[derive(Clone, Debug)]
pub enum StmtKind {
    Decl {
        name: String,
        ty: Option<TypeSpec>,
        immut: bool,
        shadow: bool,
        value: Expr,
    },
    Assign {
        name: String,
        value: Expr,
    },
    IndexAssign {
        name: String,
        name_line: usize,
        name_col: usize,
        /// Each index carries the line/col of its `[`.
        indices: Vec<(Expr, usize, usize)>,
        value: Expr,
    },
    If {
        arms: Vec<(Expr, Block)>,
        otherwise: Option<Block>,
    },
    Loop(Block),
    While(Expr, Block),
    For {
        var: String,
        iter: Expr,
        body: Block,
    },
    Break,
    Continue,
    Return(Option<Expr>),
    Check(Block),
    NoCheck(Block),
    Print {
        useful: bool,
        pieces: Vec<Piece>,
    },
    Exit(Expr),
    Call {
        name: String,
        args: Vec<Expr>,
    },
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: String,
    pub ty: Option<TypeSpec>,
    pub immut: bool,
    pub line: usize,
    pub col: usize,
}

#[derive(Clone, Debug)]
pub struct Func {
    pub name: String,
    pub line: usize,
    pub col: usize,
    pub params: Vec<Param>,
    pub ret: Option<TypeSpec>,
    pub body: Block,
}

#[derive(Clone, Debug)]
pub struct Program {
    pub funcs: Vec<Func>,
    pub main: Block,
    /// Number of expression ids handed out by the parser.
    pub expr_count: usize,
}
