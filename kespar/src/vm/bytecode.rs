//! The instruction set. A stack machine; one `Code` per function.

use crate::ast::{BinOp, BinWidth, IntWidth};
use crate::ir::{ReadBounds, SiteId};
use crate::types::Ty;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpKind { Int, Bin32, Bin64, Bool, Str }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderTy {
    Int,
    Bin32,
    Bin64,
    Bool,
    Str,
    /// A list; its element rendering is looked up at run time from the values.
    List,
}

#[derive(Debug, Clone)]
pub enum Op {
    Const(u32),
    Load(u32),
    Store(u32),
    Pop,
    /// Integer arithmetic at `width`. `overflow`/`zero` name the sites whose
    /// checks are kept; `None` means Precog proved (or the tier trusts) it.
    IntOp { op: BinOp, width: Option<IntWidth>, overflow: Option<SiteId>, second: Option<SiteId>, line: u32 },
    IntNeg { width: Option<IntWidth>, overflow: Option<SiteId>, line: u32 },
    BinOp { op: BinOp, width: BinWidth },
    BinNeg { width: BinWidth },
    Cmp { op: BinOp, kind: CmpKind },
    Not,
    Jump(u32),
    JumpIfFalse(u32),
    JumpIfTrue(u32),
    /// Pops n values, pushes a new list.
    NewList(u32),
    Fill { site: Option<SiteId>, line: u32 },
    Index { site: Option<SiteId>, line: u32 },
    /// stack: list, index, value → (nothing)
    StoreIndex { site: Option<SiteId>, line: u32 },
    Len,
    /// Pops a value, pushes its rendering.
    Render { useful: bool, ty: RenderTy },
    /// Pops n strings, pushes their concatenation.
    Concat(u32),
    /// Pops a string, writes it to stdout.
    Print,
    Read { read: u32, ty: Ty, bounds: ReadBounds, line: u32 },
    Call(u32),
    Ret,
    RetNothing,
    Exit,
    /// Integer → integer at a new width.
    IntToInt { width: IntWidth, overflow: Option<SiteId>, line: u32 },
    IntToBin { width: BinWidth },
    BinToInt { width: IntWidth, overflow: Option<SiteId>, line: u32 },
    BinToBin { width: BinWidth },
    /// Nothing to do (same type), kept so the site count is honest.
    Nop,
    /// Compile-time runs only: record the top of the stack as a value free name `n` held.
    Note(u32),
    Halt,
}

#[derive(Debug, Clone)]
pub enum Const {
    Int(i128),
    Bin64(f64),
    Bin32(f32),
    Bool(bool),
    Str(String),
}

#[derive(Debug, Clone)]
pub struct Code {
    pub name: String,
    pub nparams: u32,
    pub nlocals: u32,
    pub ops: Vec<Op>,
}

#[derive(Debug, Clone)]
pub struct Module {
    pub consts: Vec<Const>,
    pub funcs: Vec<Code>,
    pub main: u32,
    /// Read names by read id, for the contract message.
    pub read_names: Vec<String>,
    pub nfrees: u32,
}
