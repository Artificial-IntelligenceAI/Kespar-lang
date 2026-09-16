//! Layer 3: the exact question. A bounded symbolic encoding of the program
//! in Z3 bit-vectors — every path merged with `ite`, loops unrolled to a
//! bound, calls inlined to a depth — and one query per check site left
//! unproven by layer 2: can any input reach it with an illegal value?
//!
//! Sound over-approximations everywhere it gives up: a loop past the unroll
//! bound or a call past the depth *havocs* what it could have changed, and
//! any site instance inside such a loop or call is reported unproven.

use std::collections::HashMap;

use z3::ast::{Array, Ast, Bool, Dynamic, BV};
use z3::{Config, Context, Params, SatResult, Solver, Sort};

use crate::ast::{BinOp, IntWidth, Tier};
use crate::ir::*;
use crate::types::{IntTy, Ty};

/// Loop iterations encoded before the rest is havocked (provisional).
pub const UNROLL: u32 = 32;
/// Inlined call depth (provisional).
pub const DEPTH: u32 = 8;
/// Z3 resource limit per query — a step count, never a clock (provisional).
pub const RLIMIT: u32 = 10_000_000;
/// Bits used for a free name: enough that nothing Precog accepted can wrap.
const FREE_BITS: u32 = 128;

#[derive(Clone)]
enum SVal<'ctx> {
    Int(BV<'ctx>),
    /// Bins are opaque: a fresh value per operation.
    Bin(BV<'ctx>),
    Bool(Bool<'ctx>),
    /// Text is opaque except its length (scalar values).
    Str(BV<'ctx>),
    /// An object id into the heaps.
    List(BV<'ctx>),
    Nothing,
}

impl<'ctx> SVal<'ctx> {
    fn ite(c: &Bool<'ctx>, a: &SVal<'ctx>, b: &SVal<'ctx>) -> SVal<'ctx> {
        match (a, b) {
            (SVal::Int(x), SVal::Int(y)) => SVal::Int(c.ite(x, y)),
            (SVal::Bin(x), SVal::Bin(y)) => SVal::Bin(c.ite(x, y)),
            (SVal::Bool(x), SVal::Bool(y)) => SVal::Bool(c.ite(x, y)),
            (SVal::Str(x), SVal::Str(y)) => SVal::Str(c.ite(x, y)),
            (SVal::List(x), SVal::List(y)) => SVal::List(c.ite(x, y)),
            (SVal::Nothing, _) | (_, SVal::Nothing) => SVal::Nothing,
            _ => unreachable!("ite of different kinds"),
        }
    }
    fn bv(&self) -> &BV<'ctx> {
        match self {
            SVal::Int(b) | SVal::Bin(b) | SVal::Str(b) | SVal::List(b) => b,
            _ => unreachable!(),
        }
    }
    fn boolean(&self) -> &Bool<'ctx> {
        match self {
            SVal::Bool(b) => b,
            _ => unreachable!(),
        }
    }
}

#[derive(Clone)]
struct SState<'ctx> {
    vars: Vec<Option<SVal<'ctx>>>,
    /// Element sort key → array (object id → array (index → element)).
    heaps: HashMap<String, Array<'ctx>>,
    lens: Array<'ctx>,
    guard: Bool<'ctx>,
}

struct Ctl<'ctx> {
    brk: Bool<'ctx>,
    cont: Bool<'ctx>,
}

struct Instance<'ctx> {
    cond: Bool<'ctx>,
    loops: Vec<u32>,
    /// Inside a call that was cut off somewhere above it.
    in_cut: bool,
}

pub struct ReadVar<'ctx> {
    pub name: String,
    pub line: u32,
    ast: SVal<'ctx>,
    signed: bool,
}

pub struct Smt<'ctx, 'a> {
    ctx: &'ctx Context,
    prog: &'a Program,
    #[allow(dead_code)]
    widths: &'a [Option<IntWidth>],
    solver: Solver<'ctx>,
    instances: Vec<Vec<Instance<'ctx>>>,
    reads: Vec<ReadVar<'ctx>>,
    next_obj: u32,
    next_loop: u32,
    loop_stack: Vec<u32>,
    incomplete_loops: Vec<u32>,
    depth: u32,
    cut: bool,
    ret: Vec<(Bool<'ctx>, Option<SVal<'ctx>>)>,
    ctl: Vec<Ctl<'ctx>>,
    cur_func: usize,
    /// Objects with a constraint on every element: (initial row, lo, hi) for bounded reads.
    bounded: Vec<(Array<'ctx>, i128, i128)>,
    /// Objects from `std::fill`: (initial row, the value).
    fills: Vec<(Array<'ctx>, SVal<'ctx>)>,
    pub queries: u32,
}

#[derive(Debug, Clone)]
pub enum Answer {
    Proven,
    /// Some input reaches the site with an illegal value.
    Fails(String),
    Unknown(String),
}

pub fn make_context() -> Context {
    let mut cfg = Config::new();
    cfg.set_param_value("model", "true");
    Context::new(&cfg)
}

impl<'ctx, 'a> Smt<'ctx, 'a> {
    pub fn new(ctx: &'ctx Context, prog: &'a Program, widths: &'a [Option<IntWidth>]) -> Self {
        let solver = Solver::new(ctx);
        let mut params = Params::new(ctx);
        params.set_u32("rlimit", RLIMIT);
        solver.set_params(&params);
        Smt { ctx, prog, widths, solver, instances: vec![Vec::new(); prog.sites.len()].into_iter().map(|_: Vec<()>| Vec::new()).collect(), reads: Vec::new(), next_obj: 0, next_loop: 0, loop_stack: Vec::new(), incomplete_loops: Vec::new(), depth: 0, cut: false, ret: Vec::new(), ctl: Vec::new(), cur_func: prog.main as usize, bounded: Vec::new(), fills: Vec::new(), queries: 0 }
    }

    // ---- sorts and sizes ----

    fn bits(&self, ty: &Ty) -> u32 {
        match ty {
            Ty::Int(IntTy::Fixed(w)) => w.bits(),
            Ty::Int(IntTy::Free(_)) => FREE_BITS,
            _ => unreachable!("bits of {ty:?}"),
        }
    }

    fn signed(&self, ty: &Ty) -> bool {
        match ty {
            Ty::Int(IntTy::Fixed(w)) => w.signed(),
            Ty::Int(IntTy::Free(_)) => true,
            _ => true,
        }
    }

    fn elem_key(ty: &Ty) -> String {
        match ty {
            Ty::Int(IntTy::Fixed(w)) => format!("bv{}", w.bits()),
            Ty::Int(IntTy::Free(_)) => format!("bv{FREE_BITS}"),
            Ty::Bin(_) => "bv64".into(),
            Ty::Bool => "bool".into(),
            Ty::Str => "bv64".into(),
            Ty::List(_) => "bv32".into(),
            _ => unreachable!(),
        }
    }

    fn elem_sort(&self, ty: &Ty) -> Sort<'ctx> {
        match ty {
            Ty::Int(IntTy::Fixed(w)) => Sort::bitvector(self.ctx, w.bits()),
            Ty::Int(IntTy::Free(_)) => Sort::bitvector(self.ctx, FREE_BITS),
            Ty::Bin(_) | Ty::Str => Sort::bitvector(self.ctx, 64),
            Ty::Bool => Sort::bool(self.ctx),
            Ty::List(_) => Sort::bitvector(self.ctx, 32),
            _ => unreachable!(),
        }
    }

    fn heap_for(&self, st: &mut SState<'ctx>, elem: &Ty) -> Array<'ctx> {
        let key = Self::elem_key(elem);
        if let Some(h) = st.heaps.get(&key) {
            return h.clone();
        }
        let inner = Sort::array(self.ctx, &Sort::bitvector(self.ctx, 64), &self.elem_sort(elem));
        let h = Array::fresh_const(self.ctx, "heap", &Sort::bitvector(self.ctx, 32), &inner);
        st.heaps.insert(key, h.clone());
        h
    }

    fn lit(&self, v: i128, bits: u32) -> BV<'ctx> {
        if bits <= 64 {
            BV::from_i64(self.ctx, v as i64, bits)
        } else {
            BV::from_str(self.ctx, bits, &v.to_string()).unwrap_or_else(|| {
                // Negative numerals: build from the positive one.
                BV::from_str(self.ctx, bits, &(-v).to_string()).unwrap().bvneg()
            })
        }
    }

    /// Widen a value to 128 bits for width-independent comparisons.
    fn wide(&self, v: &BV<'ctx>, signed: bool) -> BV<'ctx> {
        let n = v.get_size();
        if n >= FREE_BITS {
            return v.clone();
        }
        if signed { v.sign_ext(FREE_BITS - n) } else { v.zero_ext(FREE_BITS - n) }
    }

    fn fresh(&self, ty: &Ty) -> SVal<'ctx> {
        match ty {
            Ty::Int(_) => SVal::Int(BV::fresh_const(self.ctx, "v", self.bits(ty))),
            Ty::Bin(_) => SVal::Bin(BV::fresh_const(self.ctx, "bin", 64)),
            Ty::Bool => SVal::Bool(Bool::fresh_const(self.ctx, "b")),
            Ty::Str => SVal::Str(BV::fresh_const(self.ctx, "len", 64)),
            Ty::List(_) => SVal::List(BV::fresh_const(self.ctx, "obj", 32)),
            Ty::Nothing => SVal::Nothing,
            Ty::Var(_) => unreachable!(),
        }
    }

    fn tru(&self) -> Bool<'ctx> {
        Bool::from_bool(self.ctx, true)
    }
    fn fal(&self) -> Bool<'ctx> {
        Bool::from_bool(self.ctx, false)
    }
    fn and(&self, a: &Bool<'ctx>, b: &Bool<'ctx>) -> Bool<'ctx> {
        Bool::and(self.ctx, &[a, b])
    }
    fn or(&self, a: &Bool<'ctx>, b: &Bool<'ctx>) -> Bool<'ctx> {
        Bool::or(self.ctx, &[a, b])
    }

    fn site(&mut self, st: &SState<'ctx>, id: SiteId, fail: Bool<'ctx>) {
        let cond = self.and(&st.guard, &fail);
        self.instances[id as usize].push(Instance { cond, loops: self.loop_stack.clone(), in_cut: self.cut });
    }

    // ---- entry ----

    pub fn encode(&mut self) {
        let main = &self.prog.funcs[self.prog.main as usize];
        let lens = Array::fresh_const(self.ctx, "lens", &Sort::bitvector(self.ctx, 32), &Sort::bitvector(self.ctx, 64));
        let mut st = SState { vars: vec![None; main.locals.len()], heaps: HashMap::new(), lens, guard: self.tru() };
        self.ret.push((self.fal(), None));
        self.block(&mut st, &main.body);
        self.ret.pop();
    }

    /// Ask about one site: every instance must be unsatisfiable.
    pub fn ask(&mut self, id: SiteId) -> Answer {
        let insts = std::mem::take(&mut self.instances[id as usize]);
        if insts.is_empty() {
            return Answer::Proven;
        }
        let mut incomplete = false;
        for inst in &insts {
            if inst.in_cut || inst.loops.iter().any(|l| self.incomplete_loops.contains(l)) {
                incomplete = true;
            }
            self.solver.push();
            self.solver.assert(&inst.cond);
            self.queries += 1;
            let r = self.solver.check();
            let ans = match r {
                SatResult::Unsat => None,
                SatResult::Sat => {
                    let model = self.solver.get_model();
                    let mut parts = Vec::new();
                    if let Some(m) = model {
                        for rv in &self.reads {
                            let shown = match &rv.ast {
                                SVal::Int(b) => m.eval(b, true).and_then(|v| if rv.signed { v.as_i64().map(|x| x.to_string()) } else { v.as_u64().map(|x| x.to_string()) }),
                                SVal::Bool(b) => m.eval(b, true).and_then(|v| v.as_bool().map(|x| x.to_string())),
                                SVal::Str(b) | SVal::List(b) => m.eval(b, true).and_then(|v| v.as_u64().map(|x| format!("(length {x})"))),
                                _ => None,
                            };
                            if let Some(s) = shown {
                                parts.push(format!("'{}'={}", rv.name, s));
                            }
                        }
                    }
                    Some(Answer::Fails(if parts.is_empty() { "some input".into() } else { parts.join(", ") }))
                }
                SatResult::Unknown => Some(Answer::Unknown("solver gave up".into())),
            };
            self.solver.pop(1);
            if let Some(a) = ans {
                self.instances[id as usize] = insts;
                return a;
            }
        }
        self.instances[id as usize] = insts;
        if incomplete {
            Answer::Unknown(format!("proven for the first {UNROLL} iterations only"))
        } else {
            Answer::Proven
        }
    }

    // ---- statements ----

    fn block(&mut self, st: &mut SState<'ctx>, stmts: &[Stmt]) {
        for s in stmts {
            self.stmt(st, s);
        }
    }

    fn assign(&mut self, st: &mut SState<'ctx>, v: VarId, val: SVal<'ctx>) {
        let merged = match &st.vars[v as usize] {
            Some(old) => SVal::ite(&st.guard, &val, old),
            None => val,
        };
        st.vars[v as usize] = Some(merged);
    }

    fn stmt(&mut self, st: &mut SState<'ctx>, s: &Stmt) {
        match &s.kind {
            SK::Let(v, e) | SK::Assign(v, e) => {
                let val = self.expr(st, e);
                self.assign(st, *v, val);
            }
            SK::AssignIndex(t, i, val, site) => {
                let tv = self.expr(st, t);
                let iv = self.expr(st, i);
                let vv = self.expr(st, val);
                let elem_ty = match &t.ty { Ty::List(e) => (**e).clone(), _ => unreachable!() };
                let id = tv.bv().clone();
                let (fail, idx64) = self.index_check(st, &id, &iv, &i.ty);
                self.site(st, *site, fail);
                let heap = self.heap_for(st, &elem_ty);
                let inner = heap.select(&id).as_array().unwrap();
                let stored = inner.store(&idx64, &to_dyn(&vv));
                let new_heap = heap.store(&id, &stored);
                let guarded = st.guard.ite(&new_heap, &heap);
                st.heaps.insert(Self::elem_key(&elem_ty), guarded);
            }
            SK::Print { pieces, .. } => {
                for p in pieces {
                    self.expr(st, p);
                }
            }
            SK::Exit(e) => {
                self.expr(st, e);
                st.guard = self.fal();
            }
            SK::If(branches, else_body) => {
                // `fall` guards the paths that have taken no branch yet; `taken`
                // is the merged state of every branch, live where its end was.
                let mut fall = st.guard.clone();
                let mut taken: Option<(Bool<'ctx>, SState<'ctx>)> = None;
                for (cond, body) in branches {
                    st.guard = fall.clone();
                    let c = self.expr(st, cond).boolean().clone();
                    let before = st.clone();
                    let here = self.and(&fall, &c);
                    st.guard = here.clone();
                    self.block(st, body);
                    let after = std::mem::replace(st, before);
                    fall = self.and(&fall, &c.not());
                    taken = Some(match taken {
                        None => (here, after),
                        Some((prev_c, prev)) => {
                            let merged = self.merge(&here, &after, &prev);
                            (self.or(&here, &prev_c), merged)
                        }
                    });
                }
                st.guard = fall.clone();
                if let Some(b) = else_body {
                    self.block(st, b);
                }
                if let Some((tc, ts)) = taken {
                    let merged = self.merge(&tc, &ts, st);
                    *st = merged;
                }
            }
            SK::Block(b) => self.block(st, b),
            SK::Loop(body) => self.run_loop(st, None, body),
            SK::While(cond, body) => self.run_loop(st, Some(cond), body),
            SK::ForRange { var, a, b, body } => {
                let av = self.expr(st, a);
                let bv = self.expr(st, b);
                let bits = self.bits(&a.ty);
                let signed = self.signed(&a.ty);
                let hi = BV::fresh_const(self.ctx, "hi", bits);
                self.solver.assert(&hi._eq(bv.bv()));
                let i = BV::fresh_const(self.ctx, "i", bits);
                self.solver.assert(&i._eq(av.bv()));
                self.assign(st, *var, SVal::Int(i));
                let one = self.lit(1, bits);
                let lid = self.next_loop;
                self.next_loop += 1;
                self.loop_stack.push(lid);
                let mut exit = self.fal();
                let mut fully = false;
                for _ in 0..UNROLL {
                    let cur = st.vars[*var as usize].clone().unwrap().bv().clone();
                    let cont = if signed { cur.bvsle(&hi) } else { cur.bvule(&hi) };
                    exit = self.or(&exit, &self.and(&st.guard, &cont.not()));
                    st.guard = self.and(&st.guard, &cont);
                    if !self.feasible(&st.guard) {
                        fully = true;
                        break;
                    }
                    let ctl = self.body_once(st, body);
                    exit = self.or(&exit, &ctl.brk);
                    st.guard = self.or(&st.guard, &ctl.cont);
                    // stop when i == hi, else i = i + 1
                    let cur = st.vars[*var as usize].clone().unwrap().bv().clone();
                    let last = cur._eq(&hi);
                    exit = self.or(&exit, &self.and(&st.guard, &last));
                    st.guard = self.and(&st.guard, &last.not());
                    let next = cur.bvadd(&one);
                    st.vars[*var as usize] = Some(SVal::Int(next));
                    if !self.feasible(&st.guard) {
                        fully = true;
                        break;
                    }
                }
                if !fully {
                    self.havoc(st, body);
                    exit = self.or(&exit, &st.guard);
                    self.incomplete_loops.push(lid);
                }
                self.loop_stack.pop();
                st.guard = exit;
            }
            SK::ForList { var, list, body } => {
                let lv = self.expr(st, list);
                let elem_ty = match &list.ty { Ty::List(e) => (**e).clone(), _ => unreachable!() };
                let id = lv.bv().clone();
                let len = st.lens.select(&id).as_bv().unwrap();
                let lid = self.next_loop;
                self.next_loop += 1;
                self.loop_stack.push(lid);
                let mut exit = self.fal();
                let mut fully = false;
                let mut k: u64 = 0;
                for _ in 0..UNROLL {
                    let idx = BV::from_u64(self.ctx, k, 64);
                    let cont = idx.bvult(&len);
                    exit = self.or(&exit, &self.and(&st.guard, &cont.not()));
                    st.guard = self.and(&st.guard, &cont);
                    if !self.feasible(&st.guard) {
                        fully = true;
                        break;
                    }
                    let heap = self.heap_for(st, &elem_ty);
                    let v = heap.select(&id).as_array().unwrap().select(&idx);
                    self.constrain_elem(&idx);
                    let val = self.from_dyn(&elem_ty, v);
                    self.assign(st, *var, val);
                    let ctl = self.body_once(st, body);
                    exit = self.or(&exit, &ctl.brk);
                    st.guard = self.or(&st.guard, &ctl.cont);
                    k += 1;
                }
                if !fully {
                    self.havoc(st, body);
                    exit = self.or(&exit, &st.guard);
                    self.incomplete_loops.push(lid);
                }
                self.loop_stack.pop();
                st.guard = exit;
            }
            SK::Break => {
                if let Some(top) = self.ctl.last() {
                    let brk = self.or(&top.brk, &st.guard);
                    self.ctl.last_mut().unwrap().brk = brk;
                }
                st.guard = self.fal();
            }
            SK::Continue => {
                if let Some(top) = self.ctl.last() {
                    let cont = self.or(&top.cont, &st.guard);
                    self.ctl.last_mut().unwrap().cont = cont;
                }
                st.guard = self.fal();
            }
            SK::Return(v) => {
                let val = v.as_ref().map(|e| self.expr(st, e));
                let g = st.guard.clone();
                let (rg, rv) = self.ret.last_mut().unwrap();
                *rg = Bool::or(self.ctx, &[rg, &g]);
                if let Some(val) = val {
                    *rv = Some(match rv {
                        Some(old) => SVal::ite(&g, &val, old),
                        None => val,
                    });
                }
                st.guard = self.fal();
            }
            SK::CallStmt(e) => {
                self.expr(st, e);
            }
        }
    }

    fn merge(&self, c: &Bool<'ctx>, a: &SState<'ctx>, b: &SState<'ctx>) -> SState<'ctx> {
        let vars = a.vars.iter().zip(b.vars.iter()).map(|(x, y)| match (x, y) {
            (Some(x), Some(y)) => Some(SVal::ite(c, x, y)),
            (Some(x), None) | (None, Some(x)) => Some(x.clone()),
            (None, None) => None,
        }).collect();
        let mut heaps = HashMap::new();
        for (k, ha) in &a.heaps {
            match b.heaps.get(k) {
                Some(hb) => heaps.insert(k.clone(), c.ite(ha, hb)),
                None => heaps.insert(k.clone(), ha.clone()),
            };
        }
        for (k, hb) in &b.heaps {
            heaps.entry(k.clone()).or_insert_with(|| hb.clone());
        }
        SState { vars, heaps, lens: c.ite(&a.lens, &b.lens), guard: c.ite(&a.guard, &b.guard) }
    }

    fn feasible(&mut self, g: &Bool<'ctx>) -> bool {
        self.solver.push();
        self.solver.assert(g);
        self.queries += 1;
        let r = self.solver.check();
        self.solver.pop(1);
        r != SatResult::Unsat
    }

    /// One iteration of a loop body with its own break/continue frame.
    fn body_once(&mut self, st: &mut SState<'ctx>, body: &[Stmt]) -> Ctl<'ctx> {
        self.ctl.push(Ctl { brk: self.fal(), cont: self.fal() });
        self.block(st, body);
        self.ctl.pop().unwrap()
    }

    fn run_loop(&mut self, st: &mut SState<'ctx>, cond: Option<&Expr>, body: &[Stmt]) {
        let lid = self.next_loop;
        self.next_loop += 1;
        self.loop_stack.push(lid);
        let mut exit = self.fal();
        let mut fully = false;
        for _ in 0..UNROLL {
            if let Some(c) = cond {
                let cv = self.expr(st, c).boolean().clone();
                exit = self.or(&exit, &self.and(&st.guard, &cv.not()));
                st.guard = self.and(&st.guard, &cv);
            }
            if !self.feasible(&st.guard) {
                fully = true;
                break;
            }
            let ctl = self.body_once(st, body);
            exit = self.or(&exit, &ctl.brk);
            st.guard = self.or(&st.guard, &ctl.cont);
        }
        if !fully {
            self.havoc(st, body);
            if let Some(c) = cond {
                let cv = self.expr(st, c).boolean().clone();
                exit = self.or(&exit, &self.and(&st.guard, &cv.not()));
            } else {
                exit = self.or(&exit, &st.guard);
            }
            self.incomplete_loops.push(lid);
        }
        self.loop_stack.pop();
        st.guard = exit;
    }

    /// Forget everything a loop body could have changed.
    fn havoc(&mut self, st: &mut SState<'ctx>, body: &[Stmt]) {
        let mut assigned = Vec::new();
        assigned_vars(body, &mut assigned);
        let f = &self.prog.funcs[self.cur_func];
        for v in assigned {
            let ty = f.locals[v as usize].ty.clone();
            st.vars[v as usize] = Some(self.fresh(&ty));
        }
        if writes_heap(body) {
            let keys: Vec<String> = st.heaps.keys().cloned().collect();
            for k in keys {
                let fresh = self.fresh_heap(&k);
                st.heaps.insert(k, fresh);
            }
        }
    }

    // ---- expressions ----

    fn index_check(&mut self, st: &SState<'ctx>, id: &BV<'ctx>, idx: &SVal<'ctx>, idx_ty: &Ty) -> (Bool<'ctx>, BV<'ctx>) {
        let len = st.lens.select(id).as_bv().unwrap();
        let i = self.wide(idx.bv(), self.signed(idx_ty));
        let len_w = self.wide(&len, false);
        let zero = self.lit(0, FREE_BITS);
        let fail = self.or(&i.bvslt(&zero), &i.bvsge(&len_w));
        let idx64 = i.extract(63, 0);
        (fail, idx64)
    }

    /// What is known about every element of a bounded read or a `std::fill`,
    /// asserted about the object's initial row at this index (stores build on it).
    fn constrain_elem(&mut self, idx64: &BV<'ctx>) {
        for (row, lo, hi) in self.bounded.clone() {
            let v = row.select(idx64).as_bv().unwrap();
            let bits = v.get_size();
            let (lo_b, hi_b) = (self.lit(lo, bits), self.lit(hi, bits));
            self.solver.assert(&self.and(&v.bvsge(&lo_b), &v.bvsle(&hi_b)));
        }
        for (row, val) in self.fills.clone() {
            let sel = row.select(idx64);
            let eq = match &val {
                SVal::Bool(b) => sel.as_bool().unwrap()._eq(b),
                SVal::Nothing => continue,
                other => sel.as_bv().unwrap()._eq(other.bv()),
            };
            self.solver.assert(&eq);
        }
    }

    fn from_dyn(&self, ty: &Ty, d: Dynamic<'ctx>) -> SVal<'ctx> {
        match ty {
            Ty::Int(_) => SVal::Int(d.as_bv().unwrap()),
            Ty::Bin(_) => SVal::Bin(d.as_bv().unwrap()),
            Ty::Bool => SVal::Bool(d.as_bool().unwrap()),
            Ty::Str => SVal::Str(d.as_bv().unwrap()),
            Ty::List(_) => SVal::List(d.as_bv().unwrap()),
            _ => unreachable!(),
        }
    }

    fn expr(&mut self, st: &mut SState<'ctx>, e: &Expr) -> SVal<'ctx> {
        match &e.kind {
            EK::Int(v) => match &e.ty {
                Ty::Bin(_) => SVal::Bin(BV::fresh_const(self.ctx, "bin", 64)),
                t => SVal::Int(self.lit(*v, self.bits(t))),
            },
            EK::Bin(_) => SVal::Bin(BV::fresh_const(self.ctx, "bin", 64)),
            EK::Text(s) => SVal::Str(BV::from_u64(self.ctx, s.chars().count() as u64, 64)),
            EK::Bool(b) => SVal::Bool(Bool::from_bool(self.ctx, *b)),
            EK::Var(v) => match &st.vars[*v as usize] {
                Some(x) => x.clone(),
                None => self.fresh(&e.ty),
            },
            EK::Pieces(ps) => {
                let mut len = BV::from_u64(self.ctx, 0, 64);
                for p in ps {
                    let l = match p {
                        PieceIr::Text(s) => BV::from_u64(self.ctx, s.chars().count() as u64, 64),
                        PieceIr::Newline => BV::from_u64(self.ctx, 1, 64),
                        PieceIr::Var(v, t) => {
                            let val = match &st.vars[*v as usize] { Some(x) => x.clone(), None => self.fresh(t) };
                            match val { SVal::Str(l) => l, _ => BV::fresh_const(self.ctx, "len", 64) }
                        }
                        PieceIr::Value(x) => {
                            let val = self.expr(st, x);
                            match val { SVal::Str(l) => l, _ => BV::fresh_const(self.ctx, "len", 64) }
                        }
                    };
                    len = len.bvadd(&l);
                }
                SVal::Str(len)
            }
            EK::List(items) => {
                let elem_ty = match &e.ty { Ty::List(t) => (**t).clone(), _ => unreachable!() };
                let vals: Vec<SVal<'ctx>> = items.iter().map(|i| self.expr(st, i)).collect();
                let id = self.alloc(st, &elem_ty, BV::from_u64(self.ctx, vals.len() as u64, 64));
                let heap = self.heap_for(st, &elem_ty);
                let mut inner = heap.select(&id).as_array().unwrap();
                for (k, v) in vals.iter().enumerate() {
                    inner = inner.store(&BV::from_u64(self.ctx, k as u64, 64), &to_dyn(v));
                }
                st.heaps.insert(Self::elem_key(&elem_ty), heap.store(&id, &inner));
                SVal::List(id)
            }
            EK::Neg(x, site) => {
                let v = self.expr(st, x);
                match (&v, &e.ty) {
                    (SVal::Int(b), t) => {
                        if let Some(s) = site {
                            if self.prog.sites[*s as usize].active && self.prog.sites[*s as usize].free.is_none() {
                                let ok = b.bvneg_no_overflow();
                                let ok = if self.signed(t) { ok } else { b._eq(&self.lit(0, b.get_size())) };
                                self.site(st, *s, ok.not());
                            }
                        }
                        SVal::Int(b.bvneg())
                    }
                    _ => SVal::Bin(BV::fresh_const(self.ctx, "bin", 64)),
                }
            }
            EK::Not(x) => {
                let v = self.expr(st, x);
                SVal::Bool(v.boolean().not())
            }
            EK::Binary(op, a, b, s1, s2) => self.binary(st, e, *op, a, b, *s1, *s2),
            EK::Index(l, i, site) => {
                let lv = self.expr(st, l);
                let iv = self.expr(st, i);
                let id = lv.bv().clone();
                let (fail, idx64) = self.index_check(st, &id, &iv, &i.ty);
                self.site(st, *site, fail);
                let elem_ty = match &l.ty { Ty::List(t) => (**t).clone(), _ => unreachable!() };
                let heap = self.heap_for(st, &elem_ty);
                let v = heap.select(&id).as_array().unwrap().select(&idx64);
                self.constrain_elem(&idx64);
                self.from_dyn(&elem_ty, v)
            }
            EK::Call(f, args) => {
                let vals: Vec<SVal<'ctx>> = args.iter().map(|a| self.expr(st, a)).collect();
                self.call(st, *f, vals)
            }
            EK::Read(_, bounds) => {
                let info = &self.prog.reads[match &e.kind { EK::Read(id, _) => *id as usize, _ => unreachable!() }];
                let name = info.name.clone();
                match &e.ty {
                    Ty::Int(IntTy::Fixed(w)) => {
                        let v = BV::fresh_const(self.ctx, "read", w.bits());
                        if let Some((lo, hi)) = bounds.value {
                            let (lo_b, hi_b) = (self.lit(lo, w.bits()), self.lit(hi, w.bits()));
                            let ok = if w.signed() { self.and(&v.bvsge(&lo_b), &v.bvsle(&hi_b)) } else { self.and(&v.bvuge(&lo_b), &v.bvule(&hi_b)) };
                            self.solver.assert(&ok);
                        }
                        self.reads.push(ReadVar { name, line: e.line, ast: SVal::Int(v.clone()), signed: w.signed() });
                        SVal::Int(v)
                    }
                    Ty::Bin(_) => SVal::Bin(BV::fresh_const(self.ctx, "read", 64)),
                    Ty::Bool => {
                        let v = Bool::fresh_const(self.ctx, "read");
                        self.reads.push(ReadVar { name, line: e.line, ast: SVal::Bool(v.clone()), signed: false });
                        SVal::Bool(v)
                    }
                    Ty::Str => {
                        let len = BV::fresh_const(self.ctx, "readlen", 64);
                        let (lo, hi) = bounds.len.unwrap_or((0, u32::MAX as i128));
                        self.solver.assert(&self.and(&len.bvuge(&BV::from_u64(self.ctx, lo as u64, 64)), &len.bvule(&BV::from_u64(self.ctx, hi as u64, 64))));
                        self.reads.push(ReadVar { name, line: e.line, ast: SVal::Str(len.clone()), signed: false });
                        SVal::Str(len)
                    }
                    Ty::List(elem) => {
                        let len = BV::fresh_const(self.ctx, "readlen", 64);
                        let (lo, hi) = bounds.len.unwrap_or((0, u32::MAX as i128));
                        self.solver.assert(&self.and(&len.bvuge(&BV::from_u64(self.ctx, lo as u64, 64)), &len.bvule(&BV::from_u64(self.ctx, hi as u64, 64))));
                        let id = self.alloc(st, elem, len.clone());
                        let Ty::Int(IntTy::Fixed(w)) = **elem else { unreachable!() };
                        let (elo, ehi) = bounds.value.unwrap_or((w.min(), w.max()));
                        let row = self.heap_for(st, elem).select(&id).as_array().unwrap();
                        self.bounded.push((row, elo, ehi));
                        self.reads.push(ReadVar { name, line: e.line, ast: SVal::List(len.clone()), signed: false });
                        SVal::List(id)
                    }
                    _ => unreachable!(),
                }
            }
            EK::Len(x) => {
                let v = self.expr(st, x);
                let len = match &v {
                    SVal::Str(l) => l.clone(),
                    SVal::List(id) => st.lens.select(id).as_bv().unwrap(),
                    _ => unreachable!(),
                };
                SVal::Int(len.zero_ext(FREE_BITS - 64))
            }
            EK::Fill(v, n, site) => {
                let val = self.expr(st, v);
                let nv = self.expr(st, n);
                let nw = self.wide(nv.bv(), self.signed(&n.ty));
                let fail = nw.bvslt(&self.lit(0, FREE_BITS));
                self.site(st, *site, fail);
                let elem_ty = match &e.ty { Ty::List(t) => (**t).clone(), _ => unreachable!() };
                let len = nw.extract(63, 0);
                let id = self.alloc(st, &elem_ty, len);
                // Every element is `val`: remembered, asserted lazily at each index read.
                let row = self.heap_for(st, &elem_ty).select(&id).as_array().unwrap();
                self.fills.push((row, val));
                SVal::List(id)
            }
            EK::To(x, site) => {
                let v = self.expr(st, x);
                match (&v, &x.ty, &e.ty) {
                    (SVal::Int(b), from, Ty::Int(IntTy::Fixed(w))) => {
                        let wide = self.wide(b, self.signed(from));
                        let (lo, hi) = (self.lit(IntWidth::min(*w), FREE_BITS), self.lit(IntWidth::max(*w), FREE_BITS));
                        let ok = self.and(&wide.bvsge(&lo), &wide.bvsle(&hi));
                        if let Some(s) = site {
                            self.site(st, *s, ok.not());
                        }
                        SVal::Int(wide.extract(w.bits() - 1, 0))
                    }
                    (SVal::Int(b), from, Ty::Int(IntTy::Free(_))) => SVal::Int(self.wide(b, self.signed(from))),
                    (SVal::Bin(_), _, Ty::Int(_)) => {
                        if let Some(s) = site {
                            // Bins are opaque: cannot say.
                            let unknown = Bool::fresh_const(self.ctx, "binfail");
                            self.site(st, *s, unknown);
                        }
                        self.fresh(&e.ty)
                    }
                    (_, _, Ty::Str) => SVal::Str(BV::fresh_const(self.ctx, "len", 64)),
                    (_, _, Ty::Bin(_)) => SVal::Bin(BV::fresh_const(self.ctx, "bin", 64)),
                    _ => v,
                }
            }
        }
    }

    fn fresh_heap(&self, key: &str) -> Array<'ctx> {
        let inner = Sort::array(self.ctx, &Sort::bitvector(self.ctx, 64), &self.sort_from_key(key));
        Array::fresh_const(self.ctx, "heap", &Sort::bitvector(self.ctx, 32), &inner)
    }

    fn sort_from_key(&self, key: &str) -> Sort<'ctx> {
        match key {
            "bool" => Sort::bool(self.ctx),
            k => Sort::bitvector(self.ctx, k.trim_start_matches("bv").parse().unwrap()),
        }
    }

    fn alloc(&mut self, st: &mut SState<'ctx>, _elem: &Ty, len: BV<'ctx>) -> BV<'ctx> {
        let k = self.next_obj;
        self.next_obj += 1;
        let id = BV::from_u64(self.ctx, k as u64, 32);
        st.lens = st.lens.store(&id, &len);
        id
    }

    fn call(&mut self, st: &mut SState<'ctx>, fid: FuncId, args: Vec<SVal<'ctx>>) -> SVal<'ctx> {
        let f = &self.prog.funcs[fid as usize];
        if self.depth >= DEPTH {
            self.cut = true;
            // Havoc every heap: the callee may write anything reachable.
            let keys: Vec<String> = st.heaps.keys().cloned().collect();
            for k in keys {
                let fresh = self.fresh_heap(&k);
                st.heaps.insert(k, fresh);
            }
            return self.fresh(&f.ret);
        }
        let saved_vars = std::mem::replace(&mut st.vars, vec![None; f.locals.len()]);
        for (i, a) in args.into_iter().enumerate() {
            st.vars[f.params[i] as usize] = Some(a);
        }
        let saved_guard = st.guard.clone();
        let saved_func = std::mem::replace(&mut self.cur_func, fid as usize);
        let saved_cut = self.cut;
        self.depth += 1;
        self.ret.push((self.fal(), None));
        self.block(st, &f.body);
        let (rg, rv) = self.ret.pop().unwrap();
        self.depth -= 1;
        self.cur_func = saved_func;
        st.vars = saved_vars;
        // The call returns where a `return` fired, or where the body fell off its end.
        st.guard = self.or(&rg, &st.guard);
        let _ = saved_guard;
        self.cut = saved_cut || self.cut;
        match rv {
            Some(v) => v,
            None => if f.ret == Ty::Nothing { SVal::Nothing } else { self.fresh(&f.ret) },
        }
    }

    fn binary(&mut self, st: &mut SState<'ctx>, e: &Expr, op: BinOp, a: &Expr, b: &Expr, s1: Option<SiteId>, s2: Option<SiteId>) -> SVal<'ctx> {
        match op {
            BinOp::And | BinOp::Or => {
                let av = self.expr(st, a).boolean().clone();
                // The second operand only runs where the first did not decide.
                let g = st.guard.clone();
                st.guard = if op == BinOp::And { self.and(&g, &av) } else { self.and(&g, &av.not()) };
                let bv = self.expr(st, b).boolean().clone();
                st.guard = g;
                SVal::Bool(if op == BinOp::And { self.and(&av, &bv) } else { self.or(&av, &bv) })
            }
            _ => {
                let av = self.expr(st, a);
                let bv = self.expr(st, b);
                if op.is_cmp() {
                    return SVal::Bool(match (&av, &bv) {
                        (SVal::Int(x), SVal::Int(y)) => {
                            let signed = self.signed(&a.ty);
                            match (op, signed) {
                                (BinOp::Eq, _) => x._eq(y),
                                (BinOp::Ne, _) => x._eq(y).not(),
                                (BinOp::Lt, true) => x.bvslt(y), (BinOp::Lt, false) => x.bvult(y),
                                (BinOp::Le, true) => x.bvsle(y), (BinOp::Le, false) => x.bvule(y),
                                (BinOp::Gt, true) => x.bvsgt(y), (BinOp::Gt, false) => x.bvugt(y),
                                (BinOp::Ge, true) => x.bvsge(y), (BinOp::Ge, false) => x.bvuge(y),
                                _ => unreachable!(),
                            }
                        }
                        (SVal::Bool(x), SVal::Bool(y)) => if op == BinOp::Eq { x._eq(y) } else { x._eq(y).not() },
                        _ => Bool::fresh_const(self.ctx, "cmp"),
                    });
                }
                match (&av, &bv) {
                    (SVal::Int(x), SVal::Int(y)) => {
                        let ty = &e.ty;
                        let signed = self.signed(ty);
                        let active = |s: &Option<SiteId>| s.filter(|s| self.prog.sites[*s as usize].active);
                        let r = match op {
                            BinOp::Add => {
                                if let Some(s) = active(&s1) {
                                    let ok = self.and(&x.bvadd_no_overflow(y, signed), &x.bvadd_no_underflow(y));
                                    let ok = if signed { ok } else { x.bvadd_no_overflow(y, false) };
                                    self.site(st, s, ok.not());
                                }
                                x.bvadd(y)
                            }
                            BinOp::Sub => {
                                if let Some(s) = active(&s1) {
                                    let ok = if signed { self.and(&x.bvsub_no_overflow(y), &x.bvsub_no_underflow(y, true)) } else { x.bvsub_no_underflow(y, false) };
                                    self.site(st, s, ok.not());
                                }
                                x.bvsub(y)
                            }
                            BinOp::Mul => {
                                if let Some(s) = active(&s1) {
                                    let ok = if signed { self.and(&x.bvmul_no_overflow(y, true), &x.bvmul_no_underflow(y)) } else { x.bvmul_no_overflow(y, false) };
                                    self.site(st, s, ok.not());
                                }
                                x.bvmul(y)
                            }
                            BinOp::Div | BinOp::Mod => {
                                let zero = self.lit(0, x.get_size());
                                if let Some(s) = active(&s2) {
                                    self.site(st, s, y._eq(&zero));
                                }
                                if let Some(s) = active(&s1) {
                                    let ok = if signed && op == BinOp::Div { x.bvsdiv_no_overflow(y) } else { self.tru() };
                                    self.site(st, s, ok.not());
                                }
                                // Guard the divisor so the result is well-defined on the failing path too.
                                let safe = y._eq(&zero).ite(&self.lit(1, x.get_size()), y);
                                match (op, signed) {
                                    (BinOp::Div, true) => x.bvsdiv(&safe),
                                    (BinOp::Div, false) => x.bvudiv(&safe),
                                    (BinOp::Mod, true) => x.bvsrem(&safe),
                                    (BinOp::Mod, false) => x.bvurem(&safe),
                                    _ => unreachable!(),
                                }
                            }
                            BinOp::Pow => {
                                // Not encoded: the exponent is symbolic in general.
                                if let Some(s) = active(&s2) {
                                    let zero = self.lit(0, y.get_size());
                                    let neg = if signed { y.bvslt(&zero) } else { self.fal() };
                                    self.site(st, s, neg);
                                }
                                if let Some(s) = active(&s1) {
                                    let unknown = Bool::fresh_const(self.ctx, "powfail");
                                    self.site(st, s, unknown);
                                }
                                BV::fresh_const(self.ctx, "pow", x.get_size())
                            }
                            _ => unreachable!(),
                        };
                        SVal::Int(r)
                    }
                    _ => SVal::Bin(BV::fresh_const(self.ctx, "bin", 64)),
                }
            }
        }
    }
}

fn to_dyn<'ctx>(v: &SVal<'ctx>) -> Dynamic<'ctx> {
    match v {
        SVal::Bool(b) => Dynamic::from(b),
        SVal::Nothing => unreachable!(),
        other => Dynamic::from(other.bv()),
    }
}

fn assigned_vars(stmts: &[Stmt], out: &mut Vec<VarId>) {
    for s in stmts {
        match &s.kind {
            SK::Let(v, _) | SK::Assign(v, _) => out.push(*v),
            SK::If(brs, el) => {
                for (_, b) in brs { assigned_vars(b, out); }
                if let Some(b) = el { assigned_vars(b, out); }
            }
            SK::Block(b) | SK::Loop(b) | SK::While(_, b) => assigned_vars(b, out),
            SK::ForRange { var, body, .. } | SK::ForList { var, body, .. } => { out.push(*var); assigned_vars(body, out); }
            _ => {}
        }
    }
}

fn writes_heap(stmts: &[Stmt]) -> bool {
    fn expr_calls(e: &Expr) -> bool {
        match &e.kind {
            EK::Call(..) => true,
            EK::Pieces(ps) => ps.iter().any(|p| matches!(p, PieceIr::Value(v) if expr_calls(v))),
            EK::List(es) => es.iter().any(expr_calls),
            EK::Neg(x, _) | EK::Not(x) | EK::Len(x) | EK::To(x, _) => expr_calls(x),
            EK::Binary(_, a, b, ..) | EK::Index(a, b, _) | EK::Fill(a, b, _) => expr_calls(a) || expr_calls(b),
            _ => false,
        }
    }
    stmts.iter().any(|s| match &s.kind {
        SK::AssignIndex(..) => true,
        SK::Let(_, e) | SK::Assign(_, e) | SK::Exit(e) | SK::CallStmt(e) | SK::Return(Some(e)) => expr_calls(e),
        SK::Print { pieces, .. } => pieces.iter().any(expr_calls),
        SK::If(brs, el) => brs.iter().any(|(c, b)| expr_calls(c) || writes_heap(b)) || el.as_ref().map_or(false, |b| writes_heap(b)),
        SK::Block(b) | SK::Loop(b) => writes_heap(b),
        SK::While(c, b) => expr_calls(c) || writes_heap(b),
        SK::ForRange { a, b, body, .. } => expr_calls(a) || expr_calls(b) || writes_heap(body),
        SK::ForList { list, body, .. } => expr_calls(list) || writes_heap(body),
        _ => false,
    })
}

#[allow(dead_code)]
fn tier(s: &Site) -> Tier {
    s.tier
}
