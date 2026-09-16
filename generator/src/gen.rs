//! Writes random, valid, terminating Kespar programs (design/generator.md),
//! plus the input line-sets to run them on. Everything derives from one seed.

use crate::rng::Rng;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Width { I8, I16, I32, I64, U8, U16, U32, U64 }

impl Width {
    pub const ALL: [Width; 8] = [Width::I8, Width::I16, Width::I32, Width::I64, Width::U8, Width::U16, Width::U32, Width::U64];
    pub fn name(self) -> &'static str {
        match self {
            Width::I8 => "int8", Width::I16 => "int16", Width::I32 => "int32", Width::I64 => "int64",
            Width::U8 => "uint8", Width::U16 => "uint16", Width::U32 => "uint32", Width::U64 => "uint64",
        }
    }
    pub fn min(self) -> i128 {
        match self { Width::I8 => -128, Width::I16 => -32768, Width::I32 => i32::MIN as i128, Width::I64 => i64::MIN as i128, _ => 0 }
    }
    pub fn max(self) -> i128 {
        match self {
            Width::I8 => 127, Width::I16 => 32767, Width::I32 => i32::MAX as i128, Width::I64 => i64::MAX as i128,
            Width::U8 => 255, Width::U16 => 65535, Width::U32 => u32::MAX as i128, Width::U64 => u64::MAX as i128,
        }
    }
    pub fn signed(self) -> bool {
        matches!(self, Width::I8 | Width::I16 | Width::I32 | Width::I64)
    }
}

/// The generator's view of a type. `Free` mirrors the checker's `:=`
/// inference: a free name may meet one fixed integer type, which then
/// fixes it (and everything unified with it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    Int(Width),
    /// A `:=` integer not yet forced; the id joins names that must agree.
    Free(u32),
    Bin,
    Bool,
    Str,
    List(Box<Ty>),
}

#[derive(Debug, Clone)]
pub struct Var {
    pub name: String,
    pub ty: Ty,
    pub immut: bool,
    /// A loop counter or a while-loop's own counter: never assigned.
    pub fixed: bool,
}

#[derive(Debug, Clone)]
pub struct Read {
    pub name: String,
    pub ty: Ty,
    /// Scalar or element bound.
    pub value: Option<(i128, i128)>,
    /// Length bound (str, list).
    pub len: Option<(i128, i128)>,
}

#[derive(Debug, Clone)]
pub struct FuncSig {
    pub name: String,
    pub params: Vec<Ty>,
    pub ret: Option<Ty>,
}

pub struct Case {
    pub program: String,
    pub reads: Vec<Read>,
    pub inputs: Vec<String>,
}

struct Gen {
    rng: Rng,
    out: String,
    indent: usize,
    scopes: Vec<Vec<Var>>,
    /// Union-find over free ids: parent, and a forced width if any.
    frees: Vec<(u32, Option<Width>)>,
    funcs: Vec<FuncSig>,
    reads: Vec<Read>,
    next_name: u32,
    loop_depth: u32,
    /// Variables of the enclosing while loops (their counters), never assigned by bodies.
    size: u32,
    in_func: Option<usize>,
    stmt_budget: i32,
}

pub fn case(seed: u64, size: u32) -> Case {
    let mut g = Gen { rng: Rng::new(seed), out: String::new(), indent: 0, scopes: Vec::new(), frees: Vec::new(), funcs: Vec::new(), reads: Vec::new(), next_name: 0, loop_depth: 0, size, in_func: None, stmt_budget: 0 };
    g.program();
    let inputs = g.inputs();
    Case { program: g.out, reads: g.reads, inputs }
}

impl Gen {
    // ---- helpers ----

    fn line(&mut self, s: &str) {
        for _ in 0..self.indent {
            self.out.push_str("    ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn fresh_name(&mut self) -> String {
        let names = ["a", "b", "c", "d", "e", "n", "m", "k", "total", "count", "x", "y", "z", "sum", "acc", "v", "w", "idx", "hi", "lo"];
        let base = names[self.rng.below(names.len() as u64) as usize];
        self.next_name += 1;
        format!("{base}{}", self.next_name)
    }

    fn free(&mut self) -> Ty {
        let id = self.frees.len() as u32;
        self.frees.push((id, None));
        Ty::Free(id)
    }

    fn find(&mut self, mut id: u32) -> u32 {
        while self.frees[id as usize].0 != id {
            let p = self.frees[id as usize].0;
            self.frees[id as usize].0 = self.frees[p as usize].0;
            id = p;
        }
        id
    }

    /// The type as the checker will see it now.
    fn resolve(&mut self, t: &Ty) -> Ty {
        match t {
            Ty::Free(id) => {
                let r = self.find(*id);
                match self.frees[r as usize].1 {
                    Some(w) => Ty::Int(w),
                    None => Ty::Free(r),
                }
            }
            Ty::List(e) => Ty::List(Box::new(self.resolve(e))),
            t => t.clone(),
        }
    }

    /// Make two integer types agree, the way the checker's forcing does.
    fn unify(&mut self, a: &Ty, b: &Ty) {
        let (a, b) = (self.resolve(a), self.resolve(b));
        match (&a, &b) {
            (Ty::Free(x), Ty::Free(y)) if x != y => self.frees[*y as usize].0 = *x,
            (Ty::Free(x), Ty::Int(w)) | (Ty::Int(w), Ty::Free(x)) => self.frees[*x as usize].1 = Some(*w),
            (Ty::List(x), Ty::List(y)) => self.unify(x, y),
            _ => {}
        }
    }

    fn is_int(&mut self, t: &Ty) -> bool {
        matches!(self.resolve(t), Ty::Int(_) | Ty::Free(_))
    }

    fn same(&mut self, a: &Ty, b: &Ty) -> bool {
        let (a, b) = (self.resolve(a), self.resolve(b));
        match (&a, &b) {
            (Ty::Free(_), Ty::Int(_)) | (Ty::Int(_), Ty::Free(_)) | (Ty::Free(_), Ty::Free(_)) => true,
            (Ty::List(x), Ty::List(y)) => self.same(x, y),
            _ => a == b,
        }
    }

    fn vars_of<F: Fn(&Ty) -> bool>(&mut self, f: F) -> Vec<Var> {
        let mut out = Vec::new();
        for s in &self.scopes {
            for v in s {
                out.push(v.clone());
            }
        }
        let mut keep = Vec::new();
        for v in out {
            let r = self.resolve(&v.ty);
            if f(&r) {
                keep.push(v);
            }
        }
        keep
    }

    fn declare(&mut self, v: Var) {
        self.scopes.last_mut().unwrap().push(v);
    }

    fn ty_name(&mut self, t: &Ty) -> String {
        match self.resolve(t) {
            Ty::Int(w) => w.name().to_string(),
            Ty::Free(_) => "int64".to_string(),
            Ty::Bin => "bin64".to_string(),
            Ty::Bool => "bool".to_string(),
            Ty::Str => "str.utf8".to_string(),
            Ty::List(e) => format!("list.{}", self.ty_name(&e)),
        }
    }

    // ---- program ----

    fn program(&mut self) {
        let nfuncs = self.rng.below(2 + self.size as u64 / 2) as usize;
        // Signatures first so bodies can call earlier functions.
        for i in 0..nfuncs {
            let nparams = self.rng.below(3) as usize;
            let params: Vec<Ty> = (0..nparams).map(|_| self.scalar_ty(true)).collect();
            let ret = if self.rng.chance(0.75) { Some(self.scalar_ty(true)) } else { None };
            self.funcs.push(FuncSig { name: format!("f{i}"), params, ret });
        }
        for i in 0..nfuncs {
            self.func(i);
        }
        self.line("MAIN {");
        self.indent += 1;
        self.scopes.push(Vec::new());
        self.stmt_budget = (3 + self.size * 3) as i32;
        // Reads first, at the top, in order: the input line-sets follow them.
        let nreads = self.rng.below(1 + self.size as u64) as usize;
        for _ in 0..nreads {
            self.read();
        }
        let n = 2 + self.rng.below(2 + self.size as u64 * 2) as usize;
        for _ in 0..n {
            self.stmt();
        }
        // Print something that depends on what happened.
        self.print_stmt();
        self.scopes.pop();
        self.indent -= 1;
        self.line("}");
    }

    fn scalar_ty(&mut self, allow_free: bool) -> Ty {
        match self.rng.below(if allow_free { 9 } else { 8 }) {
            0..=4 => Ty::Int(Width::ALL[self.rng.below(8) as usize]),
            5 => Ty::Bin,
            6 => Ty::Bool,
            7 => Ty::Str,
            _ => self.free(),
        }
    }

    fn func(&mut self, i: usize) {
        let sig = self.funcs[i].clone();
        self.in_func = Some(i);
        self.scopes.push(Vec::new());
        let mut params = Vec::new();
        for (k, p) in sig.params.iter().enumerate() {
            let name = format!("p{k}");
            let spelled = match p {
                Ty::Free(_) => format!("var '{name}'"),
                t => format!("var.{} '{name}'", self.ty_name(t)),
            };
            params.push(spelled);
            self.declare(Var { name, ty: p.clone(), immut: false, fixed: false });
        }
        // An untyped parameter must meet a number somewhere or the checker cannot type it.
        let mut uses = Vec::new();
        for (k, p) in sig.params.iter().enumerate() {
            if matches!(p, Ty::Free(_)) {
                uses.push(format!("var 'u{k}' := ['p{k}' + 0];"));
            }
        }
        let head = match &sig.ret {
            Some(Ty::Free(_)) | None => format!("func {} [{}] {{", sig.name, params.join(", ")),
            Some(t) => format!("func.{} {} [{}] {{", self.ty_name(t), sig.name, params.join(", ")),
        };
        self.line(&head);
        self.indent += 1;
        for u in uses {
            self.line(&u);
        }
        self.stmt_budget = (2 + self.size) as i32;
        let n = self.rng.below(1 + self.size as u64) as usize;
        for _ in 0..n {
            self.stmt();
        }
        if let Some(ret) = &sig.ret {
            let e = self.expr_of(ret, 2);
            self.line(&format!("return [{e}];"));
        }
        self.indent -= 1;
        self.line("}");
        self.scopes.pop();
        self.in_func = None;
        // Only functions before this one are callable from it; all are callable from MAIN.
    }

    fn read(&mut self) {
        let name = self.fresh_name();
        let ty = match self.rng.below(6) {
            0..=2 => Ty::Int(Width::ALL[self.rng.below(8) as usize]),
            3 => Ty::Bool,
            4 => Ty::Str,
            _ => Ty::List(Box::new(Ty::Int(Width::ALL[self.rng.below(8) as usize]))),
        };
        let (bound, value, len) = match &ty {
            Ty::Int(w) => {
                if self.rng.chance(0.7) {
                    let (lo, hi) = self.small_range(*w);
                    (format!("[{lo}, {hi}]"), Some((lo, hi)), None)
                } else {
                    ("[]".to_string(), None, None)
                }
            }
            Ty::Bool => ("[]".to_string(), None, None),
            Ty::Str => {
                if self.rng.chance(0.5) {
                    let lo = self.rng.below(3) as i128;
                    let hi = lo + self.rng.below(8) as i128;
                    (format!("[{lo}, {hi}]"), None, Some((lo, hi)))
                } else {
                    ("[]".to_string(), None, None)
                }
            }
            Ty::List(e) => {
                let Ty::Int(w) = **e else { unreachable!() };
                let lo = self.rng.below(3) as i128;
                let hi = lo + self.rng.below(6) as i128;
                if self.rng.chance(0.6) {
                    let (elo, ehi) = self.small_range(w);
                    (format!("[{lo}, {hi}, {elo}, {ehi}]"), Some((elo, ehi)), Some((lo, hi)))
                } else {
                    (format!("[{lo}, {hi}]"), None, Some((lo, hi)))
                }
            }
            _ => unreachable!(),
        };
        let tn = self.ty_name(&ty);
        self.line(&format!("var.{tn} '{name}' = [std::read.stdin{bound}];"));
        self.reads.push(Read { name: name.clone(), ty: ty.clone(), value, len });
        self.declare(Var { name, ty, immut: false, fixed: false });
    }

    fn small_range(&mut self, w: Width) -> (i128, i128) {
        let span = [10, 100, 1000, 100_000][self.rng.below(4) as usize];
        let lo = if w.signed() && self.rng.chance(0.5) { -(self.rng.below(span) as i128) } else { self.rng.below(span) as i128 };
        let lo = lo.clamp(w.min(), w.max());
        let hi = (lo + self.rng.below(span) as i128).clamp(lo, w.max());
        (lo, hi)
    }

    // ---- statements ----

    fn stmt(&mut self) {
        self.stmt_budget -= 1;
        if self.stmt_budget < 0 {
            return;
        }
        match self.rng.below(14) {
            0..=3 => self.decl_stmt(),
            4..=5 => self.assign_stmt(),
            6 => self.if_stmt(),
            7 => self.for_range_stmt(),
            8 => self.for_list_stmt(),
            9 => self.while_stmt(),
            10 => self.print_stmt(),
            11 => self.index_assign_stmt(),
            12 => self.tier_stmt(),
            _ => self.call_stmt(),
        }
    }

    fn decl_stmt(&mut self) {
        let name = self.fresh_name();
        if self.rng.chance(0.5) {
            // typed
            let ty = match self.rng.below(7) {
                0..=3 => Ty::Int(Width::ALL[self.rng.below(8) as usize]),
                4 => Ty::Bin,
                5 => Ty::Bool,
                _ => Ty::Str,
            };
            let ty = if self.rng.chance(0.2) { Ty::List(Box::new(ty)) } else { ty };
            let e = self.expr_of(&ty, 2);
            let tn = self.ty_name(&ty);
            let immut = self.rng.chance(0.15);
            let seg = if immut { ".immut" } else { "" };
            self.line(&format!("var{seg}.{tn} '{name}' = [{e}];"));
            self.declare(Var { name, ty, immut, fixed: false });
        } else {
            // inferred: the type is whatever the expression has
            let ty = match self.rng.below(6) {
                0..=2 => self.free(),
                3 => self.pick_existing_ty(),
                4 => Ty::Bool,
                _ => Ty::Str,
            };
            let e = self.expr_of(&ty, 2);
            let seg = if self.rng.chance(0.1) { ".mut" } else { "" };
            self.line(&format!("var{seg} '{name}' := [{e}];"));
            self.declare(Var { name, ty, immut: false, fixed: false });
        }
    }

    fn pick_existing_ty(&mut self) -> Ty {
        let vs = self.vars_of(|_| true);
        if vs.is_empty() {
            return self.free();
        }
        let v = &vs[self.rng.below(vs.len() as u64) as usize];
        v.ty.clone()
    }

    fn assign_stmt(&mut self) {
        let vs = self.vars_of(|_| true);
        let vs: Vec<Var> = vs.into_iter().filter(|v| !v.immut && !v.fixed).collect();
        if vs.is_empty() {
            return self.decl_stmt();
        }
        let v = vs[self.rng.below(vs.len() as u64) as usize].clone();
        let e = self.expr_of(&v.ty, 2);
        self.line(&format!("'{}' = [{e}];", v.name));
    }

    fn index_assign_stmt(&mut self) {
        let lists = self.vars_of(|t| matches!(t, Ty::List(_)));
        if lists.is_empty() {
            return self.decl_stmt();
        }
        let l = lists[self.rng.below(lists.len() as u64) as usize].clone();
        let Ty::List(elem) = self.resolve(&l.ty) else { unreachable!() };
        let i = self.index_expr();
        let e = self.expr_of(&elem, 2);
        self.line(&format!("'{}'[{i}] = [{e}];", l.name));
    }

    fn index_expr(&mut self) -> String {
        // Small non-negative indexes mostly; sometimes a variable.
        if self.rng.chance(0.7) {
            self.rng.below(4).to_string()
        } else {
            let ints = self.vars_of(|t| matches!(t, Ty::Int(_) | Ty::Free(_)));
            if ints.is_empty() {
                self.rng.below(4).to_string()
            } else {
                let v = &ints[self.rng.below(ints.len() as u64) as usize];
                format!("'{}'", v.name)
            }
        }
    }

    fn if_stmt(&mut self) {
        let c = self.expr_of(&Ty::Bool, 2);
        self.line(&format!("if [{c}] {{"));
        self.block(2);
        if self.rng.chance(0.4) {
            let c2 = self.expr_of(&Ty::Bool, 1);
            self.line(&format!("}} else.if [{c2}] {{"));
            self.block(1);
        }
        if self.rng.chance(0.6) {
            self.line("} else {");
            self.block(2);
        }
        self.line("}");
    }

    fn block(&mut self, n: u64) {
        self.indent += 1;
        self.scopes.push(Vec::new());
        let k = self.rng.below(n + 1);
        for _ in 0..k {
            self.stmt();
        }
        self.scopes.pop();
        self.indent -= 1;
    }

    fn for_range_stmt(&mut self) {
        let name = self.fresh_name();
        // Either literal bounds (free counter) or a typed read/var as the top.
        let ints = self.vars_of(|t| matches!(t, Ty::Int(_)));
        let (a, b, ty) = if !ints.is_empty() && self.rng.chance(0.5) {
            let v = ints[self.rng.below(ints.len() as u64) as usize].clone();
            let Ty::Int(w) = self.resolve(&v.ty) else { unreachable!() };
            let lo = if w.signed() { -(self.rng.below(5) as i128) } else { 0 };
            if self.rng.chance(0.5) {
                (lo.to_string(), format!("'{}'", v.name), Ty::Int(w))
            } else {
                // bounded above by a literal so it terminates quickly
                (format!("'{}'", v.name), (lo + self.rng.below(50) as i128).to_string(), Ty::Int(w))
            }
        } else {
            let lo = self.rng.below(5) as i128 - 2;
            let hi = lo + self.rng.below(40) as i128 - 2;
            (lo.to_string(), hi.to_string(), self.free())
        };
        self.line(&format!("loop.for '{name}' in [std::range[{a}, {b}]] {{"));
        self.loop_stmts(Var { name, ty, immut: true, fixed: true });
    }

    fn for_list_stmt(&mut self) {
        let lists = self.vars_of(|t| matches!(t, Ty::List(_)));
        if lists.is_empty() {
            return self.for_range_stmt();
        }
        let l = lists[self.rng.below(lists.len() as u64) as usize].clone();
        let Ty::List(elem) = self.resolve(&l.ty) else { unreachable!() };
        let name = self.fresh_name();
        self.line(&format!("loop.for '{name}' in ['{}'] {{", l.name));
        self.loop_stmts(Var { name, ty: *elem, immut: true, fixed: true });
    }

    fn while_stmt(&mut self) {
        // A private counter that only the loop's last statement changes.
        let name = self.fresh_name();
        let start = self.rng.below(12);
        let ty = if self.rng.chance(0.5) { self.free() } else { Ty::Int(Width::ALL[self.rng.below(8) as usize]) };
        let tn = self.ty_name(&ty);
        if matches!(ty, Ty::Free(_)) {
            self.line(&format!("var '{name}' := [{start}];"));
        } else {
            self.line(&format!("var.{tn} '{name}' = [{start}];"));
        }
        self.declare(Var { name: name.clone(), ty: ty.clone(), immut: false, fixed: true });
        self.line(&format!("loop.while ['{name}' > 0] {{"));
        self.indent += 1;
        self.scopes.push(Vec::new());
        self.loop_depth += 1;
        let k = self.rng.below(3);
        for _ in 0..k {
            self.stmt();
        }
        if self.rng.chance(0.2) {
            self.line(&format!("if ['{name}' == 1] {{ break; }}"));
        }
        self.line(&format!("'{name}' = ['{name}' - 1];"));
        self.loop_depth -= 1;
        self.scopes.pop();
        self.indent -= 1;
        self.line("}");
    }

    fn loop_stmts(&mut self, counter: Var) {
        self.indent += 1;
        self.scopes.push(vec![counter.clone()]);
        self.loop_depth += 1;
        let k = self.rng.below(3);
        for _ in 0..k {
            self.stmt();
        }
        if self.rng.chance(0.15) {
            let c = self.expr_of(&Ty::Bool, 1);
            let w = if self.rng.chance(0.5) { "break" } else { "continue" };
            self.line(&format!("if [{c}] {{ {w}; }}"));
        }
        self.loop_depth -= 1;
        self.scopes.pop();
        self.indent -= 1;
        self.line("}");
    }

    fn tier_stmt(&mut self) {
        // `check` around arithmetic the generator knows is safe by bounds is
        // not something it can promise, so `check` only wraps prints;
        // `nocheck` wraps nothing that can fail (a print or a bool).
        let w = if self.rng.chance(0.5) { "check" } else { "nocheck" };
        self.line(&format!("{w} {{"));
        self.indent += 1;
        self.scopes.push(Vec::new());
        self.print_stmt();
        self.scopes.pop();
        self.indent -= 1;
        self.line("}");
    }

    fn call_stmt(&mut self) {
        let callable: Vec<FuncSig> = self.callable();
        if callable.is_empty() {
            return self.print_stmt();
        }
        let f = callable[self.rng.below(callable.len() as u64) as usize].clone();
        let args = self.args_for(&f);
        self.line(&format!("{}[{}];", f.name, args.join(", ")));
    }

    fn callable(&self) -> Vec<FuncSig> {
        match self.in_func {
            Some(i) => self.funcs[..i].to_vec(),
            None => self.funcs.clone(),
        }
    }

    fn args_for(&mut self, f: &FuncSig) -> Vec<String> {
        f.params.iter().map(|p| self.expr_of(p, 1)).collect()
    }

    fn print_stmt(&mut self) {
        let useful = self.rng.chance(0.7);
        let word = if useful { "printu" } else { "print" };
        let vs = self.vars_of(|_| true);
        let mut pieces = Vec::new();
        let n = 1 + self.rng.below(3);
        for _ in 0..n {
            if !vs.is_empty() && self.rng.chance(0.7) {
                let v = &vs[self.rng.below(vs.len() as u64) as usize];
                pieces.push(format!("'{}'", v.name));
            } else {
                pieces.push(format!("\"{}\"", ["a", "b ", "x=", ",", "ok"][self.rng.below(5) as usize]));
            }
            if self.rng.chance(0.3) {
                pieces.push("\" \"".to_string());
            }
        }
        pieces.push("\\n".to_string());
        self.line(&format!("std::{word}.stdout[{}];", pieces.join(" ")));
    }

    // ---- expressions ----

    /// An expression of type `t`, fully parenthesised where operators nest.
    fn expr_of(&mut self, t: &Ty, depth: u32) -> String {
        let r = self.resolve(t);
        match r {
            Ty::Int(_) | Ty::Free(_) => self.int_expr(t, depth),
            Ty::Bin => self.bin_expr(depth),
            Ty::Bool => self.bool_expr(depth),
            Ty::Str => self.str_expr(depth),
            Ty::List(e) => self.list_expr(&e, depth),
        }
    }

    fn int_lit(&mut self, t: &Ty) -> String {
        let r = self.resolve(t);
        let v: i128 = match r {
            Ty::Int(w) => {
                // `-128` is negation of 128, which does not fit int8: stay above the minimum.
                let (lo, hi) = ((w.min() + 1).max(-1000), w.max().min(1000));
                lo + self.rng.below((hi - lo + 1) as u64) as i128
            }
            // A free name may later be forced to any width: stay inside int8.
            _ => self.rng.below(128) as i128 - if self.rng.chance(0.3) { 60 } else { 0 },
        };
        if v < 0 { format!("(-{})", -v) } else { v.to_string() }
    }

    fn int_var(&mut self, t: &Ty) -> Option<String> {
        let cands = self.vars_of(|x| matches!(x, Ty::Int(_) | Ty::Free(_)));
        let mut ok = Vec::new();
        for v in cands {
            if self.same(&v.ty, t) {
                ok.push(v);
            }
        }
        if ok.is_empty() {
            return None;
        }
        let v = ok[self.rng.below(ok.len() as u64) as usize].clone();
        self.unify(&v.ty, t);
        Some(format!("'{}'", v.name))
    }

    fn int_expr(&mut self, t: &Ty, depth: u32) -> String {
        if depth == 0 || self.rng.chance(0.3) {
            if self.rng.chance(0.6) {
                if let Some(v) = self.int_var(t) {
                    return v;
                }
            }
            return self.int_lit(t);
        }
        match self.rng.below(12) {
            0..=5 => {
                let op = ["+", "-", "x", "/", "mod", "xx"][self.rng.below(6) as usize];
                let a = self.int_expr(t, depth - 1);
                let b = if op == "xx" { self.rng.below(4).to_string() } else { self.int_expr(t, depth - 1) };
                format!("({a} {op} {b})")
            }
            6 => {
                let a = self.int_expr(t, depth - 1);
                format!("(-{a})")
            }
            7 => {
                // an element of a list of this type
                let lists = self.vars_of(|x| matches!(x, Ty::List(_)));
                let mut ok = Vec::new();
                for l in lists {
                    let Ty::List(e) = self.resolve(&l.ty) else { continue };
                    if self.same(&e, t) {
                        ok.push((l, *e));
                    }
                }
                if ok.is_empty() {
                    return self.int_lit(t);
                }
                let (l, e) = ok[self.rng.below(ok.len() as u64) as usize].clone();
                self.unify(&e, t);
                let i = self.index_expr();
                format!("'{}'[{i}]", l.name)
            }
            8 => {
                // std::len adopts the context type
                let ls = self.vars_of(|x| matches!(x, Ty::List(_) | Ty::Str));
                if ls.is_empty() {
                    return self.int_lit(t);
                }
                let v = &ls[self.rng.below(ls.len() as u64) as usize];
                format!("std::len['{}']", v.name)
            }
            9 => {
                // conversion into a fixed width
                let Ty::Int(w) = self.resolve(t) else { return self.int_lit(t) };
                let src = self.pick_existing_ty();
                let src_r = self.resolve(&src);
                match src_r {
                    Ty::Int(_) | Ty::Free(_) | Ty::Bin => {
                        let e = self.expr_of(&src, depth - 1);
                        format!("std::to.{}[{e}]", w.name())
                    }
                    _ => self.int_lit(t),
                }
            }
            10 => {
                // a call returning this type
                let callable = self.callable();
                let mut ok = Vec::new();
                for f in callable {
                    if let Some(r) = &f.ret {
                        if self.is_int(r) && self.same(r, t) {
                            ok.push(f);
                        }
                    }
                }
                if ok.is_empty() {
                    return self.int_lit(t);
                }
                let f = ok[self.rng.below(ok.len() as u64) as usize].clone();
                self.unify(f.ret.as_ref().unwrap(), t);
                let args = self.args_for(&f);
                format!("{}[{}]", f.name, args.join(", "))
            }
            _ => self.int_var(t).unwrap_or_else(|| self.int_lit(t)),
        }
    }

    fn bin_expr(&mut self, depth: u32) -> String {
        if depth == 0 || self.rng.chance(0.4) {
            let vs = self.vars_of(|x| *x == Ty::Bin);
            if !vs.is_empty() && self.rng.chance(0.6) {
                let v = &vs[self.rng.below(vs.len() as u64) as usize];
                return format!("'{}'", v.name);
            }
            let whole = self.rng.below(100);
            let frac = self.rng.below(100);
            return format!("{whole}.{frac:02}");
        }
        match self.rng.below(4) {
            0..=2 => {
                let op = ["+", "-", "x", "/"][self.rng.below(4) as usize];
                let a = self.bin_expr(depth - 1);
                let b = self.bin_expr(depth - 1);
                format!("({a} {op} {b})")
            }
            _ => {
                let src = self.pick_existing_ty();
                if self.is_int(&src) {
                    let e = self.expr_of(&src, depth - 1);
                    format!("std::to.bin64[{e}]")
                } else {
                    self.bin_expr(0)
                }
            }
        }
    }

    fn bool_expr(&mut self, depth: u32) -> String {
        if depth == 0 || self.rng.chance(0.25) {
            let vs = self.vars_of(|x| *x == Ty::Bool);
            if !vs.is_empty() && self.rng.chance(0.5) {
                let v = &vs[self.rng.below(vs.len() as u64) as usize];
                return format!("'{}'", v.name);
            }
            return if self.rng.chance(0.5) { "true".into() } else { "false".into() };
        }
        match self.rng.below(6) {
            0..=2 => {
                let op = ["==", "!==", "<", ">", "<==", ">=="][self.rng.below(6) as usize];
                let t = self.pick_existing_ty();
                let r = self.resolve(&t);
                let t = match r {
                    Ty::Int(_) | Ty::Free(_) | Ty::Bin => t,
                    Ty::Bool if op == "==" || op == "!==" => t,
                    Ty::Str if op == "==" || op == "!==" => {
                        // pieces cannot sit inside `( )`: compare simple text values
                        let a = self.str_simple();
                        let b = self.str_simple();
                        return format!("({a} {op} {b})");
                    }
                    _ => self.free(),
                };
                let a = self.expr_of(&t, depth - 1);
                let b = self.expr_of(&t, depth - 1);
                format!("({a} {op} {b})")
            }
            3 => {
                let a = self.bool_expr(depth - 1);
                let b = self.bool_expr(depth - 1);
                let op = if self.rng.chance(0.5) { "and" } else { "or" };
                format!("({a} {op} {b})")
            }
            4 => {
                let a = self.bool_expr(depth - 1);
                format!("(not {a})")
            }
            _ => {
                let callable = self.callable();
                let ok: Vec<FuncSig> = callable.into_iter().filter(|f| f.ret == Some(Ty::Bool)).collect();
                if ok.is_empty() {
                    return self.bool_expr(0);
                }
                let f = ok[self.rng.below(ok.len() as u64) as usize].clone();
                let args = self.args_for(&f);
                format!("{}[{}]", f.name, args.join(", "))
            }
        }
    }

    fn str_simple(&mut self) -> String {
        let vs = self.vars_of(|x| *x == Ty::Str);
        if !vs.is_empty() && self.rng.chance(0.6) {
            let v = &vs[self.rng.below(vs.len() as u64) as usize];
            return format!("'{}'", v.name);
        }
        format!("\"{}\"", ["hi", "", "x y"][self.rng.below(3) as usize])
    }

    fn str_expr(&mut self, depth: u32) -> String {
        let vs = self.vars_of(|_| true);
        if depth == 0 || vs.is_empty() || self.rng.chance(0.4) {
            return format!("\"{}\"", ["hi", "", "x y", "Kespar", "a,b"][self.rng.below(5) as usize]);
        }
        // pieces: text and names; at least one text so a lone name is not mistaken for the name itself
        let mut ps = vec![format!("\"{}\"", ["a", "-", " "][self.rng.below(3) as usize])];
        let n = 1 + self.rng.below(3);
        for _ in 0..n {
            if self.rng.chance(0.5) {
                let v = &vs[self.rng.below(vs.len() as u64) as usize];
                ps.push(format!("'{}'", v.name));
            } else {
                ps.push(format!("\"{}\"", ["a", "-", " "][self.rng.below(3) as usize]));
            }
        }
        if self.rng.chance(0.2) {
            let src = self.pick_existing_ty();
            let r = self.resolve(&src);
            if matches!(r, Ty::Int(_) | Ty::Free(_) | Ty::Bin | Ty::Bool) {
                let e = self.expr_of(&src, 1);
                return format!("std::to.str.utf8[{e}]");
            }
        }
        ps.join(" ")
    }

    fn list_expr(&mut self, elem: &Ty, depth: u32) -> String {
        let lists = self.vars_of(|x| matches!(x, Ty::List(_)));
        let mut same = Vec::new();
        for l in lists {
            let Ty::List(e) = self.resolve(&l.ty) else { continue };
            if self.same(&e, elem) {
                same.push((l, *e));
            }
        }
        if !same.is_empty() && self.rng.chance(0.3) {
            let (l, e) = same[self.rng.below(same.len() as u64) as usize].clone();
            self.unify(&e, elem);
            return format!("'{}'", l.name);
        }
        if self.rng.chance(0.3) {
            let v = self.expr_of(elem, depth.saturating_sub(1));
            let n = self.rng.below(6);
            return format!("std::fill[{v}, {n}]");
        }
        let n = 1 + self.rng.below(4);
        let items: Vec<String> = (0..n).map(|_| self.expr_of(elem, depth.saturating_sub(1))).collect();
        if items.len() == 1 {
            format!("[{},]", items[0])
        } else {
            format!("[{}]", items.join(", "))
        }
    }

    // ---- inputs ----

    fn inputs(&mut self) -> Vec<String> {
        if self.reads.is_empty() {
            return vec![String::new()];
        }
        let mut sets = Vec::new();
        // Random within bounds.
        for _ in 0..4 {
            let mut lines = Vec::new();
            for r in self.reads.clone() {
                lines.push(self.random_line(&r));
            }
            sets.push(lines.join("\n") + "\n");
        }
        // Edges: every read at each of its edge values, others random.
        for (k, r) in self.reads.clone().iter().enumerate() {
            for edge in self.edge_lines(r) {
                let mut lines = Vec::new();
                for (j, r2) in self.reads.clone().iter().enumerate() {
                    lines.push(if j == k { edge.clone() } else { self.random_line(r2) });
                }
                sets.push(lines.join("\n") + "\n");
            }
        }
        sets
    }

    fn random_line(&mut self, r: &Read) -> String {
        match &r.ty {
            Ty::Int(w) => {
                let (lo, hi) = r.value.unwrap_or((w.min(), w.max()));
                self.rand_in(lo, hi).to_string()
            }
            Ty::Bool => if self.rng.chance(0.5) { "true".into() } else { "false".into() },
            Ty::Str => {
                let (lo, hi) = r.len.unwrap_or((0, 12));
                let n = self.rand_in(lo, hi) as usize;
                let alphabet = ['a', 'b', ' ', 'z', '9', 'é', '-'];
                (0..n).map(|_| alphabet[self.rng.below(alphabet.len() as u64) as usize]).collect()
            }
            Ty::List(e) => {
                let Ty::Int(w) = **e else { unreachable!() };
                let (lo, hi) = r.len.unwrap_or((0, 8));
                let n = self.rand_in(lo, hi) as usize;
                let (elo, ehi) = r.value.unwrap_or((w.min(), w.max()));
                (0..n).map(|_| self.rand_in(elo, ehi).to_string()).collect::<Vec<_>>().join(" ")
            }
            _ => unreachable!(),
        }
    }

    fn rand_in(&mut self, lo: i128, hi: i128) -> i128 {
        if hi <= lo {
            return lo;
        }
        let span = (hi - lo) as u128;
        if span > u64::MAX as u128 {
            // Wide: pick from a few interesting spots.
            let picks = [lo, hi, 0, -1, 1, lo / 2, hi / 2];
            return picks[self.rng.below(picks.len() as u64) as usize].clamp(lo, hi);
        }
        lo + (self.rng.below(span as u64 + 1) as i128)
    }

    fn edge_lines(&mut self, r: &Read) -> Vec<String> {
        match &r.ty {
            Ty::Int(w) => {
                let (lo, hi) = r.value.unwrap_or((w.min(), w.max()));
                let mut v = vec![lo, hi, 0, 1, -1, lo + 1, hi - 1];
                v.retain(|x| *x >= lo && *x <= hi);
                v.dedup();
                v.iter().map(|x| x.to_string()).collect()
            }
            Ty::Bool => vec!["true".into(), "false".into()],
            Ty::Str => {
                let (lo, hi) = r.len.unwrap_or((0, 12));
                vec!["a".repeat(lo as usize), "z".repeat(hi as usize)]
            }
            Ty::List(e) => {
                let Ty::Int(w) = **e else { unreachable!() };
                let (lo, hi) = r.len.unwrap_or((0, 8));
                let (elo, ehi) = r.value.unwrap_or((w.min(), w.max()));
                vec![
                    vec![elo.to_string(); lo as usize].join(" "),
                    vec![ehi.to_string(); hi as usize].join(" "),
                    (0..hi).map(|i| if i % 2 == 0 { elo } else { ehi }.to_string()).collect::<Vec<_>>().join(" "),
                ]
            }
            _ => unreachable!(),
        }
    }
}
