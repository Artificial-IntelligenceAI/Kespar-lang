//! IR → bytecode, taking Precog's decisions: which sites keep their check,
//! which width each free name has, which expressions are known constants,
//! and — for a program with no reads — the whole output, already computed.

use std::collections::HashMap;

use crate::ast::{BinOp, BinWidth, IntWidth};
use crate::ir::*;
use crate::types::{IntTy, Ty};
use crate::vm::bytecode::*;
use crate::vm::Outcome;

/// A known scalar value Precog folded.
#[derive(Debug, Clone, PartialEq)]
pub enum Known {
    Int(i128),
    Bin64(f64),
    Bin32(f32),
    Bool(bool),
    Str(String),
}

#[derive(Debug, Clone, Default)]
pub struct Decisions {
    /// Per site id: keep the run-time check?
    pub site_checked: Vec<bool>,
    /// Per free id: the width Precog chose.
    pub widths: Vec<IntWidth>,
    /// Expression id → known value (only for pure expressions).
    pub known: HashMap<u32, Known>,
    /// The program read nothing and was run to the end at compile time:
    /// its stdout and how it ended.
    pub whole: Option<(Vec<u8>, Outcome)>,
}

struct Emitter<'a> {
    prog: &'a Program,
    dec: &'a Decisions,
    consts: Vec<Const>,
    ops: Vec<Op>,
    extra: u32,
    nlocals: u32,
    /// (break patch sites, continue target or patch sites) per enclosing loop.
    loops: Vec<LoopCtx>,
}

struct LoopCtx {
    breaks: Vec<usize>,
    continues: Vec<usize>,
}

pub fn emit(prog: &Program, dec: &Decisions) -> Module {
    let mut consts = Vec::new();
    let mut funcs = Vec::new();

    if let Some((out, outcome)) = &dec.whole {
        // The whole program ran at compile time: MAIN is its output.
        for f in &prog.funcs {
            funcs.push(Code { name: f.name.clone(), nparams: f.params.len() as u32, nlocals: 0, ops: vec![Op::RetNothing] });
        }
        let mut ops = Vec::new();
        if !out.is_empty() {
            consts.push(Const::Str(String::from_utf8_lossy(out).into_owned()));
            ops.push(Op::Const(0));
            ops.push(Op::Print);
        }
        match outcome {
            Outcome::Exit(0) => ops.push(Op::Halt),
            Outcome::Exit(n) => {
                consts.push(Const::Int(*n as i128));
                ops.push(Op::Const((consts.len() - 1) as u32));
                ops.push(Op::Exit);
            }
            _ => ops.push(Op::Halt),
        }
        funcs[prog.main as usize] = Code { name: "MAIN".into(), nparams: 0, nlocals: 0, ops };
        return Module { consts, funcs, main: prog.main, read_names: prog.reads.iter().map(|r| r.name.clone()).collect() };
    }

    for f in &prog.funcs {
        let mut em = Emitter { prog, dec, consts: std::mem::take(&mut consts), ops: Vec::new(), extra: 0, nlocals: f.locals.len() as u32, loops: Vec::new() };
        em.block(&f.body);
        em.ops.push(Op::RetNothing);
        let nlocals = em.nlocals + em.extra;
        consts = em.consts;
        funcs.push(Code { name: f.name.clone(), nparams: f.params.len() as u32, nlocals, ops: em.ops });
    }
    Module { consts, funcs, main: prog.main, read_names: prog.reads.iter().map(|r| r.name.clone()).collect() }
}

impl<'a> Emitter<'a> {
    fn width(&self, t: &Ty) -> IntWidth {
        match t {
            Ty::Int(IntTy::Fixed(w)) => *w,
            Ty::Int(IntTy::Free(id)) => self.dec.widths[*id as usize],
            other => unreachable!("width of {other:?}"),
        }
    }

    fn keep(&self, site: Option<SiteId>) -> Option<SiteId> {
        let s = site?;
        let st = &self.prog.sites[s as usize];
        if st.active && self.dec.site_checked[s as usize] { Some(s) } else { None }
    }

    fn konst(&mut self, c: Const) {
        self.consts.push(c);
        self.ops.push(Op::Const((self.consts.len() - 1) as u32));
    }

    fn slot(&mut self) -> u32 {
        let s = self.nlocals + self.extra;
        self.extra += 1;
        s
    }

    fn here(&self) -> u32 {
        self.ops.len() as u32
    }

    fn patch(&mut self, at: usize, target: u32) {
        match &mut self.ops[at] {
            Op::Jump(t) | Op::JumpIfFalse(t) | Op::JumpIfTrue(t) => *t = target,
            _ => unreachable!(),
        }
    }

    fn block(&mut self, stmts: &[Stmt]) {
        for s in stmts {
            self.stmt(s);
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match &s.kind {
            SK::Let(v, e) | SK::Assign(v, e) => {
                self.expr(e);
                self.ops.push(Op::Store(*v));
            }
            SK::AssignIndex(t, i, val, site) => {
                self.expr(t);
                self.expr(i);
                self.expr(val);
                let site = self.keep(Some(*site));
                self.ops.push(Op::StoreIndex { site, line: s.line });
            }
            SK::Print { useful, pieces } => {
                let n = pieces.len() as u32;
                for p in pieces {
                    match &p.kind {
                        EK::Pieces(ps) => self.pieces(ps, *useful),
                        _ => {
                            self.expr(p);
                            if p.ty != Ty::Str {
                                self.ops.push(Op::Render { useful: *useful, ty: render_ty(&p.ty) });
                            }
                        }
                    }
                }
                if n != 1 {
                    self.ops.push(Op::Concat(n));
                }
                self.ops.push(Op::Print);
            }
            SK::Exit(e) => {
                self.expr(e);
                self.ops.push(Op::Exit);
            }
            SK::If(branches, else_body) => {
                let mut end_jumps = Vec::new();
                let mut done = false;
                for (cond, body) in branches {
                    if done {
                        break;
                    }
                    match self.dec.known.get(&cond.id) {
                        Some(Known::Bool(true)) => {
                            self.block(body);
                            done = true;
                        }
                        Some(Known::Bool(false)) => {}
                        _ => {
                            self.expr(cond);
                            let jf = self.ops.len();
                            self.ops.push(Op::JumpIfFalse(0));
                            self.block(body);
                            end_jumps.push(self.ops.len());
                            self.ops.push(Op::Jump(0));
                            let h = self.here();
                            self.patch(jf, h);
                        }
                    }
                }
                if !done {
                    if let Some(b) = else_body {
                        self.block(b);
                    }
                }
                let h = self.here();
                for j in end_jumps {
                    self.patch(j, h);
                }
            }
            SK::Block(b) => self.block(b),
            SK::Loop(body) => {
                let start = self.here();
                self.loops.push(LoopCtx { breaks: Vec::new(), continues: Vec::new() });
                self.block(body);
                self.ops.push(Op::Jump(start));
                self.end_loop(start);
            }
            SK::While(cond, body) => {
                let start = self.here();
                self.expr(cond);
                let jf = self.ops.len();
                self.ops.push(Op::JumpIfFalse(0));
                self.loops.push(LoopCtx { breaks: Vec::new(), continues: Vec::new() });
                self.block(body);
                self.ops.push(Op::Jump(start));
                let end = self.here();
                self.patch(jf, end);
                self.end_loop(start);
            }
            SK::ForRange { var, a, b, body } => {
                let hi = self.slot();
                self.expr(a);
                self.ops.push(Op::Store(*var));
                self.expr(b);
                self.ops.push(Op::Store(hi));
                // if a > b → end
                self.ops.push(Op::Load(*var));
                self.ops.push(Op::Load(hi));
                self.ops.push(Op::Cmp { op: BinOp::Gt, kind: CmpKind::Int });
                let j_empty = self.ops.len();
                self.ops.push(Op::JumpIfTrue(0));
                let body_start = self.here();
                self.loops.push(LoopCtx { breaks: Vec::new(), continues: Vec::new() });
                self.block(body);
                // continue target: if i == hi → end; i = i + 1; jump body
                let cont = self.here();
                self.ops.push(Op::Load(*var));
                self.ops.push(Op::Load(hi));
                self.ops.push(Op::Cmp { op: BinOp::Eq, kind: CmpKind::Int });
                let j_done = self.ops.len();
                self.ops.push(Op::JumpIfTrue(0));
                self.ops.push(Op::Load(*var));
                self.konst(Const::Int(1));
                let w = self.width(&a.ty);
                self.ops.push(Op::IntOp { op: BinOp::Add, width: w, overflow: None, second: None, line: s.line });
                self.ops.push(Op::Store(*var));
                self.ops.push(Op::Jump(body_start));
                let end = self.here();
                self.patch(j_empty, end);
                self.patch(j_done, end);
                self.end_loop(cont);
            }
            SK::ForList { var, list, body } => {
                let l = self.slot();
                let idx = self.slot();
                self.expr(list);
                self.ops.push(Op::Store(l));
                self.konst(Const::Int(0));
                self.ops.push(Op::Store(idx));
                let start = self.here();
                self.ops.push(Op::Load(idx));
                self.ops.push(Op::Load(l));
                self.ops.push(Op::Len);
                self.ops.push(Op::Cmp { op: BinOp::Lt, kind: CmpKind::Int });
                let jf = self.ops.len();
                self.ops.push(Op::JumpIfFalse(0));
                self.ops.push(Op::Load(l));
                self.ops.push(Op::Load(idx));
                self.ops.push(Op::Index { site: None, line: s.line });
                self.ops.push(Op::Store(*var));
                self.loops.push(LoopCtx { breaks: Vec::new(), continues: Vec::new() });
                self.block(body);
                let cont = self.here();
                self.ops.push(Op::Load(idx));
                self.konst(Const::Int(1));
                self.ops.push(Op::IntOp { op: BinOp::Add, width: IntWidth::I64, overflow: None, second: None, line: s.line });
                self.ops.push(Op::Store(idx));
                self.ops.push(Op::Jump(start));
                let end = self.here();
                self.patch(jf, end);
                self.end_loop(cont);
            }
            SK::Break => {
                let at = self.ops.len();
                self.ops.push(Op::Jump(0));
                self.loops.last_mut().unwrap().breaks.push(at);
            }
            SK::Continue => {
                let at = self.ops.len();
                self.ops.push(Op::Jump(0));
                self.loops.last_mut().unwrap().continues.push(at);
            }
            SK::Return(None) => self.ops.push(Op::RetNothing),
            SK::Return(Some(e)) => {
                self.expr(e);
                self.ops.push(Op::Ret);
            }
            SK::CallStmt(e) => {
                self.expr(e);
                if e.ty != Ty::Nothing {
                    self.ops.push(Op::Pop);
                }
            }
        }
    }

    fn end_loop(&mut self, cont_target: u32) {
        let ctx = self.loops.pop().unwrap();
        let end = self.here();
        for b in ctx.breaks {
            self.patch(b, end);
        }
        for c in ctx.continues {
            self.patch(c, cont_target);
        }
    }

    fn pieces(&mut self, ps: &[PieceIr], useful: bool) {
        for p in ps {
            match p {
                PieceIr::Text(s) => self.konst(Const::Str(s.clone())),
                PieceIr::Newline => self.konst(Const::Str("\n".into())),
                PieceIr::Value(v) => {
                    self.expr(v);
                    if v.ty != Ty::Str {
                        self.ops.push(Op::Render { useful, ty: render_ty(&v.ty) });
                    }
                }
                PieceIr::Var(v, t) => {
                    self.ops.push(Op::Load(*v));
                    if *t != Ty::Str {
                        self.ops.push(Op::Render { useful, ty: render_ty(t) });
                    }
                }
            }
        }
        if ps.len() != 1 {
            self.ops.push(Op::Concat(ps.len() as u32));
        }
    }

    fn expr(&mut self, e: &Expr) {
        if let Some(k) = self.dec.known.get(&e.id) {
            match k {
                Known::Int(v) => self.konst(Const::Int(*v)),
                Known::Bin64(f) => self.konst(Const::Bin64(*f)),
                Known::Bin32(f) => self.konst(Const::Bin32(*f)),
                Known::Bool(b) => self.konst(Const::Bool(*b)),
                Known::Str(s) => self.konst(Const::Str(s.clone())),
            }
            return;
        }
        match &e.kind {
            EK::Int(v) => match &e.ty {
                Ty::Bin(BinWidth::B64) => self.konst(Const::Bin64(*v as f64)),
                Ty::Bin(BinWidth::B32) => self.konst(Const::Bin32(*v as f32)),
                _ => self.konst(Const::Int(*v)),
            },
            EK::Bin(f) => match &e.ty {
                Ty::Bin(BinWidth::B32) => self.konst(Const::Bin32(*f as f32)),
                _ => self.konst(Const::Bin64(*f)),
            },
            EK::Text(s) => self.konst(Const::Str(s.clone())),
            EK::Bool(b) => self.konst(Const::Bool(*b)),
            EK::Var(v) => self.ops.push(Op::Load(*v)),
            EK::Pieces(ps) => self.pieces(ps, false),
            EK::List(items) => {
                for i in items {
                    self.expr(i);
                }
                self.ops.push(Op::NewList(items.len() as u32));
            }
            EK::Neg(x, site) => {
                self.expr(x);
                match &e.ty {
                    Ty::Bin(w) => self.ops.push(Op::BinNeg { width: *w }),
                    t => {
                        let width = self.width(t);
                        let overflow = self.keep(*site);
                        self.ops.push(Op::IntNeg { width, overflow, line: e.line });
                    }
                }
            }
            EK::Not(x) => {
                self.expr(x);
                self.ops.push(Op::Not);
            }
            EK::Binary(op, a, b, s1, s2) => match op {
                BinOp::And => {
                    self.expr(a);
                    let j = self.ops.len();
                    self.ops.push(Op::JumpIfFalse(0));
                    self.expr(b);
                    let j2 = self.ops.len();
                    self.ops.push(Op::Jump(0));
                    let f = self.here();
                    self.patch(j, f);
                    self.konst(Const::Bool(false));
                    let end = self.here();
                    self.patch(j2, end);
                }
                BinOp::Or => {
                    self.expr(a);
                    let j = self.ops.len();
                    self.ops.push(Op::JumpIfTrue(0));
                    self.expr(b);
                    let j2 = self.ops.len();
                    self.ops.push(Op::Jump(0));
                    let t = self.here();
                    self.patch(j, t);
                    self.konst(Const::Bool(true));
                    let end = self.here();
                    self.patch(j2, end);
                }
                op if op.is_cmp() => {
                    self.expr(a);
                    self.expr(b);
                    let kind = match &a.ty {
                        Ty::Int(_) => CmpKind::Int,
                        Ty::Bin(BinWidth::B64) => CmpKind::Bin64,
                        Ty::Bin(BinWidth::B32) => CmpKind::Bin32,
                        Ty::Bool => CmpKind::Bool,
                        Ty::Str => CmpKind::Str,
                        other => unreachable!("cmp on {other:?}"),
                    };
                    self.ops.push(Op::Cmp { op: *op, kind });
                }
                op => {
                    self.expr(a);
                    self.expr(b);
                    match &e.ty {
                        Ty::Bin(w) => self.ops.push(Op::BinOp { op: *op, width: *w }),
                        t => {
                            let width = self.width(t);
                            let overflow = self.keep(*s1);
                            let second = self.keep(*s2);
                            self.ops.push(Op::IntOp { op: *op, width, overflow, second, line: e.line });
                        }
                    }
                }
            },
            EK::Index(l, i, site) => {
                self.expr(l);
                self.expr(i);
                let site = self.keep(Some(*site));
                self.ops.push(Op::Index { site, line: e.line });
            }
            EK::Call(f, args) => {
                for a in args {
                    self.expr(a);
                }
                self.ops.push(Op::Call(*f));
            }
            EK::Read(id, bounds) => {
                self.ops.push(Op::Read { read: *id, ty: e.ty.clone(), bounds: bounds.clone(), line: e.line });
            }
            EK::Len(x) => {
                self.expr(x);
                self.ops.push(Op::Len);
            }
            EK::Fill(v, n, site) => {
                self.expr(v);
                self.expr(n);
                let site = self.keep(Some(*site));
                self.ops.push(Op::Fill { site, line: e.line });
            }
            EK::To(x, site) => {
                self.expr(x);
                match (&x.ty, &e.ty) {
                    (Ty::Int(_), Ty::Int(_)) => {
                        let width = self.width(&e.ty);
                        let overflow = self.keep(*site);
                        self.ops.push(Op::IntToInt { width, overflow, line: e.line });
                    }
                    (Ty::Int(_), Ty::Bin(w)) => self.ops.push(Op::IntToBin { width: *w }),
                    (Ty::Bin(_), Ty::Int(_)) => {
                        let width = self.width(&e.ty);
                        let overflow = self.keep(*site);
                        self.ops.push(Op::BinToInt { width, overflow, line: e.line });
                    }
                    (Ty::Bin(_), Ty::Bin(w)) => self.ops.push(Op::BinToBin { width: *w }),
                    (Ty::Str, Ty::Str) => {}
                    (t, Ty::Str) => self.ops.push(Op::Render { useful: true, ty: render_ty(t) }),
                    _ => self.ops.push(Op::Nop),
                }
            }
        }
    }
}

fn render_ty(t: &Ty) -> RenderTy {
    match t {
        Ty::Int(_) => RenderTy::Int,
        Ty::Bin(BinWidth::B32) => RenderTy::Bin32,
        Ty::Bin(BinWidth::B64) => RenderTy::Bin64,
        Ty::Bool => RenderTy::Bool,
        Ty::Str => RenderTy::Str,
        Ty::List(_) => RenderTy::List,
        _ => RenderTy::Str,
    }
}
