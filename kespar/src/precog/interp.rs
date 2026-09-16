//! Layer 2: the abstract interpreter. Runs the program over sets of values,
//! exactly where it can (known values, bounded counted loops trip by trip)
//! and by widening where it cannot. Records, per check site, whether every
//! value that reached it was legal; per free name, every value it held;
//! per expression, its value if it is always the same.

use std::collections::{BTreeSet, HashMap};

use crate::ast::{BinOp, BinWidth, IntWidth, Tier};
use crate::diag::{CompileError, Result};
use crate::ir::*;
use crate::types::{IntTy, Ty};
use crate::vm::render;

use super::domain::*;

/// Trips a counted loop is followed one by one before widening (provisional).
pub const UNROLL_CAP: u32 = 100_000;
/// Iterations of a `loop.while` followed one by one before widening (provisional).
pub const WHILE_CAP: u32 = 64;
/// Nested call depth before a call is cut off and its result taken as anything (provisional).
pub const CALL_DEPTH: u32 = 8;

#[derive(Debug, Clone, PartialEq)]
pub struct State {
    pub vars: Vec<AVal>,
    pub heap: Vec<AList>,
}

impl State {
    fn join(&self, o: &State) -> State {
        let vars = self.vars.iter().zip(o.vars.iter()).map(|(a, b)| a.join(b)).collect();
        let n = self.heap.len().max(o.heap.len());
        let mut heap = Vec::with_capacity(n);
        for i in 0..n {
            match (self.heap.get(i), o.heap.get(i)) {
                (Some(a), Some(b)) => heap.push(a.join(b)),
                (Some(a), None) | (None, Some(a)) => heap.push(a.clone()),
                _ => unreachable!(),
            }
        }
        State { vars, heap }
    }
}

fn join_opt(a: Option<State>, b: Option<State>) -> Option<State> {
    match (a, b) {
        (None, x) | (x, None) => x,
        (Some(a), Some(b)) => Some(a.join(&b)),
    }
}

#[derive(Default)]
struct Out {
    next: Option<State>,
    brk: Option<State>,
    cont: Option<State>,
    ret: Option<(State, AVal)>,
}

impl Out {
    fn merge(&mut self, o: Out) {
        self.next = join_opt(self.next.take(), o.next);
        self.brk = join_opt(self.brk.take(), o.brk);
        self.cont = join_opt(self.cont.take(), o.cont);
        self.ret = match (self.ret.take(), o.ret) {
            (None, x) | (x, None) => x,
            (Some((s1, v1)), Some((s2, v2))) => Some((s1.join(&s2), v1.join(&v2))),
        };
    }
}

#[derive(Debug, Clone)]
pub struct SiteState {
    pub reached: bool,
    /// Every evaluation so far proved the property.
    pub proven: bool,
    /// Some evaluation could only fail.
    pub certain: bool,
    /// The widest thing seen, for the report.
    pub note: String,
    /// Every integer result that passed through (overflow sites), for the report.
    pub seen: ISet,
}

pub struct Analysis<'a> {
    pub prog: &'a Program,
    /// Allocation site (expr id) → abstract object.
    alloc: HashMap<u32, u32>,
    pub sites: Vec<SiteState>,
    pub results: HashMap<u32, AVal>,
    pub frees: Vec<ISet>,
    pub steps: u64,
    pub budget: u64,
    depth: u32,
    /// Functions whose analysis was cut short (recursion depth): nothing inside them is trusted.
    pub incomplete: Vec<bool>,
    /// The line of the loop being followed, for the budget message.
    loop_line: u32,
    /// Something in the current function hit the step budget.
    exhausted: Option<u32>,
    cur_func: FuncId,
    /// Which function each expression belongs to.
    pub expr_func: HashMap<u32, FuncId>,
}

fn top_of(ty: &Ty) -> AVal {
    match ty {
        Ty::Int(IntTy::Fixed(w)) => AVal::Int(ISet::of_width(*w)),
        Ty::Int(IntTy::Free(_)) => AVal::Int(ISet::unbounded()),
        Ty::Bin(_) => AVal::Bin(ABin::Unknown),
        Ty::Bool => AVal::Bool(BSet::BOTH),
        Ty::Str => AVal::Str(AStr { known: None, len: ISet::Range(0, POS_INF) }),
        Ty::List(_) => AVal::List(AListRef(BTreeSet::new())),
        Ty::Nothing => AVal::Nothing,
        Ty::Var(_) => unreachable!(),
    }
}

fn elem_ty(ty: &Ty) -> &Ty {
    match ty {
        Ty::List(e) => e,
        _ => unreachable!(),
    }
}

/// Does evaluating this expression do anything besides compute a value?
pub fn pure(e: &Expr) -> bool {
    match &e.kind {
        EK::Read(..) | EK::Call(..) => false,
        EK::Int(_) | EK::Bin(_) | EK::Text(_) | EK::Bool(_) | EK::Var(_) => true,
        EK::Pieces(ps) => ps.iter().all(|p| match p { PieceIr::Value(v) => pure(v), _ => true }),
        EK::List(es) => es.iter().all(pure),
        EK::Neg(x, _) | EK::Not(x) | EK::Len(x) | EK::To(x, _) => pure(x),
        EK::Binary(_, a, b, ..) | EK::Index(a, b, _) | EK::Fill(a, b, _) => pure(a) && pure(b),
    }
}

impl<'a> Analysis<'a> {
    pub fn new(prog: &'a Program, budget: u64) -> Self {
        let sites = prog.sites.iter().map(|_| SiteState { reached: false, proven: true, certain: false, note: String::new(), seen: ISet::Empty }).collect();
        let mut a = Analysis { prog, alloc: HashMap::new(), sites, results: HashMap::new(), frees: vec![ISet::Empty; prog.free_names.len()], steps: 0, budget, depth: 0, incomplete: vec![false; prog.funcs.len()], loop_line: 0, exhausted: None, cur_func: prog.main, expr_func: HashMap::new() };
        for (i, f) in prog.funcs.iter().enumerate() {
            let mut ids = Vec::new();
            collect_exprs(&f.body, &mut ids);
            for id in ids {
                a.expr_func.insert(id, i as FuncId);
            }
        }
        a
    }

    /// Run `MAIN` abstractly from nothing known.
    pub fn run(&mut self) -> Result<()> {
        let main = &self.prog.funcs[self.prog.main as usize];
        let st = State { vars: vec![AVal::Undef; main.locals.len()], heap: Vec::new() };
        let _ = self.exec_func(self.prog.main, st, Vec::new());
        if let Some(line) = self.exhausted {
            return Err(CompileError::new(line, format!("compile-time analysis exceeded {} steps in the loop at line {line}", self.budget)));
        }
        Ok(())
    }

    fn tick(&mut self) {
        self.steps += 1;
        if self.steps > self.budget && self.exhausted.is_none() {
            self.exhausted = Some(self.loop_line);
        }
    }

    fn out_of_budget(&self) -> bool {
        self.exhausted.is_some()
    }

    fn site(&mut self, id: SiteId, proven: bool, certain: bool, note: String) {
        let s = &mut self.sites[id as usize];
        s.reached = true;
        if !proven {
            s.proven = false;
            if s.note.is_empty() || certain {
                s.note = note;
            }
        } else if s.note.is_empty() {
            s.note = note;
        }
        if certain {
            s.certain = true;
        }
    }

    fn note_free(&mut self, ty: &Ty, v: &AVal) {
        if let (Ty::Int(IntTy::Free(id)), AVal::Int(s)) = (ty, v) {
            let id = *id as usize;
            self.frees[id] = self.frees[id].join(s);
        }
    }

    fn record(&mut self, e: &Expr, v: &AVal) {
        self.note_free(&e.ty, v);
        if !matches!(v, AVal::List(_) | AVal::Nothing | AVal::Undef) && pure(e) {
            let joined = match self.results.get(&e.id) {
                Some(old) => old.join(v),
                None => v.clone(),
            };
            self.results.insert(e.id, joined);
        }
    }

    // ---- functions ----

    /// Returns the state after the call (heap carried back) and the value.
    fn exec_func(&mut self, fid: FuncId, mut st: State, args: Vec<AVal>) -> (State, AVal) {
        let f = &self.prog.funcs[fid as usize];
        for (i, a) in args.into_iter().enumerate() {
            let p = f.params[i];
            self.note_free(&f.locals[p as usize].ty, &a);
            st.vars[p as usize] = a;
        }
        self.depth += 1;
        let saved = self.cur_func;
        self.cur_func = fid;
        let out = self.exec_block(st, &f.body);
        self.cur_func = saved;
        self.depth -= 1;
        let (mut state, mut val) = (None, AVal::Undef);
        if let Some(s) = out.next {
            state = Some(s);
            val = AVal::Nothing;
        }
        if let Some((s, v)) = out.ret {
            state = join_opt(state, Some(s));
            val = if matches!(val, AVal::Undef) { v } else { val.join(&v) };
        }
        let state = state.unwrap_or_else(|| State { vars: Vec::new(), heap: Vec::new() });
        (state, val)
    }

    fn call(&mut self, st: &mut State, fid: FuncId, args: Vec<AVal>) -> AVal {
        let f = &self.prog.funcs[fid as usize];
        if self.depth >= CALL_DEPTH || self.out_of_budget() {
            // Cut off: the result is anything, and whatever the callee could
            // reach through its list arguments is anything too.
            self.incomplete[fid as usize] = true;
            self.mark_incomplete_callees(fid);
            for a in &args {
                self.top_reachable(st, a);
            }
            return top_of(&f.ret);
        }
        let callee = State { vars: vec![AVal::Undef; f.locals.len()], heap: std::mem::take(&mut st.heap) };
        let (after, v) = self.exec_func(fid, callee, args);
        if !after.heap.is_empty() || v != AVal::Undef {
            st.heap = after.heap;
        }
        if v == AVal::Undef {
            // No path returned: the call never comes back (always exits or loops forever).
            return AVal::Undef;
        }
        v
    }

    fn mark_incomplete_callees(&mut self, fid: FuncId) {
        // Every function reachable from `fid` may have been skipped.
        let mut stack = vec![fid];
        let mut seen = vec![false; self.prog.funcs.len()];
        while let Some(f) = stack.pop() {
            if seen[f as usize] {
                continue;
            }
            seen[f as usize] = true;
            self.incomplete[f as usize] = true;
            let mut callees = Vec::new();
            collect_calls(&self.prog.funcs[f as usize].body, &mut callees);
            stack.extend(callees);
        }
    }

    fn top_reachable(&mut self, st: &mut State, v: &AVal) {
        if let AVal::List(r) = v {
            let ids: Vec<u32> = r.0.iter().copied().collect();
            for id in ids {
                let obj = &mut st.heap[id as usize];
                let inner = obj.elem.clone();
                obj.len = ISet::Range(0, POS_INF);
                obj.exact = None;
                obj.single = false;
                obj.elem = match &inner {
                    AVal::Int(_) => AVal::Int(ISet::unbounded()),
                    AVal::Bin(_) => AVal::Bin(ABin::Unknown),
                    AVal::Bool(_) => AVal::Bool(BSet::BOTH),
                    AVal::Str(_) => AVal::Str(AStr { known: None, len: ISet::Range(0, POS_INF) }),
                    other => other.clone(),
                };
                if let AVal::List(_) = inner {
                    self.top_reachable(st, &inner);
                }
            }
        }
    }

    // ---- statements ----

    fn exec_block(&mut self, st: State, stmts: &[Stmt]) -> Out {
        let mut cur = Some(st);
        let mut out = Out::default();
        for s in stmts {
            let Some(st) = cur.take() else { break };
            if self.out_of_budget() {
                out.next = Some(st);
                return out;
            }
            let o = self.exec(st, s);
            cur = o.next;
            out.merge(Out { next: None, brk: o.brk, cont: o.cont, ret: o.ret });
        }
        out.next = cur;
        out
    }

    fn exec(&mut self, mut st: State, s: &Stmt) -> Out {
        self.tick();
        let mut out = Out::default();
        match &s.kind {
            SK::Let(v, e) | SK::Assign(v, e) => {
                let val = self.eval(&mut st, e);
                if val == AVal::Undef {
                    return out; // the value never arrives (a call that never returns)
                }
                let t = self.prog.funcs[self.cur_func as usize].locals[*v as usize].ty.clone();
                self.note_free(&t, &val);
                st.vars[*v as usize] = val;
                out.next = Some(st);
            }
            SK::AssignIndex(target, idx, val, site) => {
                let t = self.eval(&mut st, target);
                let i = self.eval(&mut st, idx);
                let v = self.eval(&mut st, val);
                if t == AVal::Undef || i == AVal::Undef || v == AVal::Undef {
                    return out;
                }
                let AVal::List(refs) = &t else { unreachable!() };
                let iset = i.as_int().clone();
                let (proven, certain, note) = self.bounds_check(&st, refs, &iset);
                self.site(*site, proven, certain, note);
                if certain {
                    return out;
                }
                for id in refs.0.iter().copied() {
                    let obj = &mut st.heap[id as usize];
                    let in_range = iset.clip(0, obj.len.hi().saturating_sub(1));
                    match (&mut obj.exact, in_range.single(), obj.single) {
                        (Some(ex), Some(k), true) if (k as usize) < ex.len() => {
                            ex[k as usize] = v.clone();
                            obj.elem = ex.iter().fold(AVal::Undef, |a, b| a.join(b));
                        }
                        (Some(ex), _, _) => {
                            match in_range.iter_values() {
                                Some(vals) => {
                                    for k in vals {
                                        if (k as usize) < ex.len() {
                                            ex[k as usize] = ex[k as usize].join(&v);
                                        }
                                    }
                                }
                                None => {
                                    for x in ex.iter_mut() {
                                        *x = x.join(&v);
                                    }
                                }
                            }
                            obj.elem = obj.elem.join(&v);
                        }
                        (None, _, _) => obj.elem = obj.elem.join(&v),
                    }
                }
                out.next = Some(st);
            }
            SK::Print { pieces, .. } => {
                for p in pieces {
                    let v = self.eval(&mut st, p);
                    if v == AVal::Undef {
                        return out;
                    }
                }
                out.next = Some(st);
            }
            SK::Exit(e) => {
                let _ = self.eval(&mut st, e);
                // No path continues.
            }
            SK::If(branches, else_body) => {
                let mut cur = Some(st);
                for (cond, body) in branches {
                    let Some(mut s) = cur.take() else { break };
                    let c = self.eval(&mut s, cond);
                    if c == AVal::Undef {
                        break;
                    }
                    let b = c.as_bool();
                    if b.can_true() {
                        let ts = self.narrow(&s, cond, true);
                        let o = self.exec_block(ts, body);
                        out.merge(o);
                    }
                    if b.can_false() {
                        cur = Some(self.narrow(&s, cond, false));
                    }
                }
                if let Some(s) = cur {
                    match else_body {
                        Some(b) => {
                            let o = self.exec_block(s, b);
                            out.merge(o);
                        }
                        None => out.next = join_opt(out.next.take(), Some(s)),
                    }
                }
            }
            SK::Block(b) => return self.exec_block(st, b),
            SK::Loop(body) => {
                let saved = self.loop_line;
                self.loop_line = s.line;
                let o = self.run_loop(st, None, body, s.line);
                self.loop_line = saved;
                return o;
            }
            SK::While(cond, body) => {
                let saved = self.loop_line;
                self.loop_line = s.line;
                let o = self.run_loop(st, Some(cond), body, s.line);
                self.loop_line = saved;
                return o;
            }
            SK::ForRange { var, a, b, body } => {
                let saved = self.loop_line;
                self.loop_line = s.line;
                let o = self.for_range(st, *var, a, b, body);
                self.loop_line = saved;
                return o;
            }
            SK::ForList { var, list, body } => {
                let saved = self.loop_line;
                self.loop_line = s.line;
                let o = self.for_list(st, *var, list, body);
                self.loop_line = saved;
                return o;
            }
            SK::Break => out.brk = Some(st),
            SK::Continue => out.cont = Some(st),
            SK::Return(None) => out.ret = Some((st, AVal::Nothing)),
            SK::Return(Some(e)) => {
                let v = self.eval(&mut st, e);
                if v == AVal::Undef {
                    return out;
                }
                out.ret = Some((st, v));
            }
            SK::CallStmt(e) => {
                let v = self.eval(&mut st, e);
                if v == AVal::Undef && matches!(e.kind, EK::Call(..)) && e.ty != Ty::Nothing {
                    return out;
                }
                if v == AVal::Undef && e.ty == Ty::Nothing {
                    // a nothing-function that never returns
                    return out;
                }
                out.next = Some(st);
            }
        }
        out
    }

    /// Generic loop: `loop { }` (cond None) and `loop.while [cond] { }`.
    fn run_loop(&mut self, entry: State, cond: Option<&Expr>, body: &[Stmt], _line: u32) -> Out {
        let mut out = Out::default();
        let mut exit: Option<State> = None;
        let mut head = entry;
        let mut prev_head: Option<State> = None;
        let mut iter = 0u32;
        loop {
            if self.out_of_budget() {
                out.next = join_opt(exit, Some(head));
                return out;
            }
            // Evaluate the condition.
            let body_state = match cond {
                None => Some(head.clone()),
                Some(c) => {
                    let mut s = head.clone();
                    let cv = self.eval(&mut s, c);
                    if cv == AVal::Undef {
                        break;
                    }
                    let b = cv.as_bool();
                    if b.can_false() {
                        exit = join_opt(exit, Some(self.narrow(&s, c, false)));
                    }
                    if b.can_true() { Some(self.narrow(&s, c, true)) } else { None }
                }
            };
            let Some(bs) = body_state else { break };
            let o = self.exec_block(bs, body);
            exit = join_opt(exit, o.brk);
            if let Some(r) = o.ret {
                out.ret = match out.ret.take() {
                    None => Some(r),
                    Some((s, v)) => Some((s.join(&r.0), v.join(&r.1))),
                };
            }
            let Some(next) = join_opt(o.next, o.cont) else { break };
            iter += 1;
            if iter <= WHILE_CAP {
                // Follow it exactly: the next head is exactly the state after this trip.
                head = next;
                continue;
            }
            // Widen: join with the previous head and push growing ends to the limits.
            let joined = match &prev_head {
                None => head.join(&next),
                Some(p) => p.join(&next),
            };
            let widened = self.widen_state(prev_head.as_ref().unwrap_or(&head), &joined);
            if Some(&widened) == prev_head.as_ref() || widened == head {
                // Stable: one more pass through the condition for the exit state.
                if let Some(c) = cond {
                    let mut s = widened.clone();
                    let cv = self.eval(&mut s, c);
                    if cv != AVal::Undef && cv.as_bool().can_false() {
                        exit = join_opt(exit, Some(self.narrow(&s, c, false)));
                    }
                }
                break;
            }
            prev_head = Some(widened.clone());
            head = widened;
        }
        out.next = exit;
        out
    }

    fn widen_state(&self, old: &State, new: &State) -> State {
        let vars = old.vars.iter().zip(new.vars.iter()).map(|(o, n)| widen_val(o, n)).collect();
        let n = new.heap.len();
        let mut heap = Vec::with_capacity(n);
        for i in 0..n {
            match old.heap.get(i) {
                Some(o) => {
                    let nw = &new.heap[i];
                    heap.push(AList { len: o.len.widen(&nw.len, 0, POS_INF), elem: widen_val(&o.elem, &nw.elem), exact: nw.exact.clone().filter(|_| o.exact.is_some()), single: nw.single && o.single });
                }
                None => heap.push(new.heap[i].clone()),
            }
        }
        State { vars, heap }
    }

    fn for_range(&mut self, mut entry: State, var: VarId, a: &Expr, b: &Expr, body: &[Stmt]) -> Out {
        let mut out = Out::default();
        let av = self.eval(&mut entry, a);
        let bv = self.eval(&mut entry, b);
        if av == AVal::Undef || bv == AVal::Undef {
            return out;
        }
        let aset = av.as_int().clone();
        let bset = bv.as_int().clone();
        let var_ty = a.ty.clone();
        let mut exit: Option<State> = None;
        // Zero trips if some a > b.
        if aset.hi() > bset.lo() {
            exit = Some(entry.clone());
        }
        let trips_bounded = aset.bounded() && bset.bounded() && (bset.hi() - aset.lo()) < UNROLL_CAP as i128;
        if trips_bounded {
            let mut st = entry;
            let mut k: i128 = 0;
            loop {
                if self.out_of_budget() {
                    out.next = join_opt(exit, Some(st));
                    return out;
                }
                let ik = aset.binop(BinOp::Add, &ISet::one(k)).clip(NEG_INF, bset.hi());
                if ik.is_empty() {
                    break;
                }
                // The loop runs trip k only where a + k <= b.
                let mut s = st.clone();
                s.vars[var as usize] = AVal::Int(ik.clone());
                self.note_free(&var_ty, &AVal::Int(ik.clone()));
                let o = self.exec_block(s, body);
                exit = join_opt(exit, o.brk);
                if let Some(r) = o.ret {
                    out.ret = match out.ret.take() {
                        None => Some(r),
                        Some((s, v)) => Some((s.join(&r.0), v.join(&r.1))),
                    };
                }
                let Some(next) = join_opt(o.next, o.cont) else { break };
                // Trip k was the last where a + k == b is possible.
                let ends = aset.binop(BinOp::Add, &ISet::one(k)).narrow(BinOp::Eq, &bset, true);
                if !ends.is_empty() {
                    exit = join_opt(exit, Some(next.clone()));
                }
                st = next;
                k += 1;
            }
            out.next = exit;
            return out;
        }
        // Too many trips: the counter is the whole interval and the body runs to a fixpoint.
        let irange = ISet::Range(aset.lo(), bset.hi());
        let mut head = entry;
        head.vars[var as usize] = AVal::Int(irange.clone());
        self.note_free(&var_ty, &AVal::Int(irange.clone()));
        let mut prev: Option<State> = None;
        loop {
            if self.out_of_budget() {
                out.next = join_opt(exit, Some(head));
                return out;
            }
            let o = self.exec_block(head.clone(), body);
            exit = join_opt(exit, o.brk);
            if let Some(r) = o.ret {
                out.ret = match out.ret.take() {
                    None => Some(r),
                    Some((s, v)) => Some((s.join(&r.0), v.join(&r.1))),
                };
            }
            let Some(mut next) = join_opt(o.next, o.cont) else { break };
            exit = join_opt(exit, Some(next.clone()));
            next.vars[var as usize] = AVal::Int(irange.clone());
            let joined = head.join(&next);
            let widened = match &prev {
                None => joined,
                Some(p) => self.widen_state(p, &joined),
            };
            if widened == head {
                break;
            }
            prev = Some(head);
            head = widened;
        }
        out.next = exit;
        out
    }

    fn for_list(&mut self, mut entry: State, var: VarId, list: &Expr, body: &[Stmt]) -> Out {
        let mut out = Out::default();
        let lv = self.eval(&mut entry, list);
        if lv == AVal::Undef {
            return out;
        }
        let AVal::List(refs) = &lv else { unreachable!() };
        let var_ty = elem_ty(&list.ty).clone();
        let mut exit: Option<State> = None;
        // Exactly one object with per-element values: trip by trip.
        if refs.0.len() == 1 {
            let id = *refs.0.iter().next().unwrap();
            if let Some(ex) = entry.heap[id as usize].exact.clone() {
                let mut st = entry;
                for (k, ev) in ex.iter().enumerate() {
                    if self.out_of_budget() {
                        out.next = join_opt(exit, Some(st));
                        return out;
                    }
                    // Re-read the element each trip: the body may have written it.
                    let cur = st.heap[id as usize].exact.as_ref().map(|e| e[k].clone()).unwrap_or_else(|| ev.clone());
                    let mut s = st.clone();
                    s.vars[var as usize] = cur.clone();
                    self.note_free(&var_ty, &cur);
                    let o = self.exec_block(s, body);
                    exit = join_opt(exit, o.brk);
                    if let Some(r) = o.ret {
                        out.ret = match out.ret.take() {
                            None => Some(r),
                            Some((s, v)) => Some((s.join(&r.0), v.join(&r.1))),
                        };
                    }
                    let Some(next) = join_opt(o.next, o.cont) else {
                        out.next = exit;
                        return out;
                    };
                    st = next;
                }
                out.next = join_opt(exit, Some(st));
                return out;
            }
        }
        // Summary: the element is anything the list holds; run to a fixpoint.
        let mut len = ISet::Empty;
        let mut elem = AVal::Undef;
        for id in refs.0.iter() {
            let o = &entry.heap[*id as usize];
            len = len.join(&o.len);
            elem = elem.join(&o.elem);
        }
        if len.contains(0) || len.is_empty() || elem == AVal::Undef {
            exit = Some(entry.clone());
        }
        if elem == AVal::Undef || len.hi() == 0 {
            out.next = exit;
            return out;
        }
        // A bounded length: follow the trips one by one, each with the element summary;
        // the loop may end after any trip count the length allows.
        if len.bounded() && len.hi() <= UNROLL_CAP as i128 {
            let mut st = entry;
            let mut k: i128 = 1;
            while k <= len.hi() {
                if self.out_of_budget() {
                    out.next = join_opt(exit, Some(st));
                    return out;
                }
                let mut elem = AVal::Undef;
                for id in refs.0.iter() {
                    elem = elem.join(&st.heap[*id as usize].elem);
                }
                let mut s = st.clone();
                s.vars[var as usize] = elem.clone();
                self.note_free(&var_ty, &elem);
                let o = self.exec_block(s, body);
                exit = join_opt(exit, o.brk);
                if let Some(r) = o.ret {
                    out.ret = match out.ret.take() {
                        None => Some(r),
                        Some((s, v)) => Some((s.join(&r.0), v.join(&r.1))),
                    };
                }
                let Some(next) = join_opt(o.next, o.cont) else { break };
                if len.contains(k) {
                    exit = join_opt(exit, Some(next.clone()));
                }
                st = next;
                k += 1;
            }
            out.next = exit;
            return out;
        }
        let mut head = entry;
        let mut prev: Option<State> = None;
        loop {
            if self.out_of_budget() {
                out.next = join_opt(exit, Some(head));
                return out;
            }
            let mut elem = AVal::Undef;
            for id in refs.0.iter() {
                elem = elem.join(&head.heap[*id as usize].elem);
            }
            head.vars[var as usize] = elem.clone();
            self.note_free(&var_ty, &elem);
            let o = self.exec_block(head.clone(), body);
            exit = join_opt(exit, o.brk);
            if let Some(r) = o.ret {
                out.ret = match out.ret.take() {
                    None => Some(r),
                    Some((s, v)) => Some((s.join(&r.0), v.join(&r.1))),
                };
            }
            let Some(next) = join_opt(o.next, o.cont) else { break };
            exit = join_opt(exit, Some(next.clone()));
            let joined = head.join(&next);
            let widened = match &prev {
                None => joined,
                Some(p) => self.widen_state(p, &joined),
            };
            if widened == head {
                break;
            }
            prev = Some(head);
            head = widened;
        }
        out.next = exit;
        out
    }

    // ---- narrowing on conditions ----

    fn narrow(&mut self, st: &State, cond: &Expr, truth: bool) -> State {
        let mut s = st.clone();
        self.narrow_into(&mut s, cond, truth);
        s
    }

    fn narrow_into(&mut self, s: &mut State, cond: &Expr, truth: bool) {
        match &cond.kind {
            EK::Not(x) => self.narrow_into(s, x, !truth),
            EK::Binary(BinOp::And, a, b, ..) if truth => {
                self.narrow_into(s, a, true);
                self.narrow_into(s, b, true);
            }
            EK::Binary(BinOp::Or, a, b, ..) if !truth => {
                self.narrow_into(s, a, false);
                self.narrow_into(s, b, false);
            }
            EK::Binary(BinOp::And, a, b, ..) => {
                // not (a and b): either side may be false
                let mut s1 = s.clone();
                self.narrow_into(&mut s1, a, false);
                let mut s2 = s.clone();
                self.narrow_into(&mut s2, b, false);
                *s = s1.join(&s2);
            }
            EK::Binary(BinOp::Or, a, b, ..) => {
                let mut s1 = s.clone();
                self.narrow_into(&mut s1, a, true);
                let mut s2 = s.clone();
                self.narrow_into(&mut s2, b, true);
                *s = s1.join(&s2);
            }
            EK::Binary(op, a, b, ..) if op.is_cmp() && a.ty.is_int() => {
                let av = self.peek_int(s, a);
                let bv = self.peek_int(s, b);
                if let (Some(av), Some(bv)) = (av, bv) {
                    if let EK::Var(v) = a.kind {
                        s.vars[v as usize] = AVal::Int(av.narrow(*op, &bv, truth));
                    }
                    if let EK::Var(v) = b.kind {
                        let flipped = match op {
                            BinOp::Lt => BinOp::Gt, BinOp::Gt => BinOp::Lt, BinOp::Le => BinOp::Ge, BinOp::Ge => BinOp::Le,
                            o => *o,
                        };
                        s.vars[v as usize] = AVal::Int(bv.narrow(flipped, &av, truth));
                    }
                }
            }
            EK::Var(v) if cond.ty == Ty::Bool => {
                s.vars[*v as usize] = AVal::Bool(BSet::of(truth));
            }
            _ => {}
        }
    }

    /// The set of a pure integer expression without recording anything.
    fn peek_int(&mut self, s: &State, e: &Expr) -> Option<ISet> {
        if !pure(e) {
            return None;
        }
        let saved_results = self.results.clone();
        let saved_sites = self.sites.clone();
        let saved_frees = self.frees.clone();
        let saved_steps = self.steps;
        let mut tmp = s.clone();
        let v = self.eval(&mut tmp, e);
        self.results = saved_results;
        self.sites = saved_sites;
        self.frees = saved_frees;
        self.steps = saved_steps;
        match v {
            AVal::Int(i) => Some(i),
            _ => None,
        }
    }

    // ---- expressions ----

    fn width_of(&self, ty: &Ty) -> Option<IntWidth> {
        match ty {
            Ty::Int(IntTy::Fixed(w)) => Some(*w),
            _ => None,
        }
    }

    fn tier(&self, site: SiteId) -> Tier {
        self.prog.sites[site as usize].tier
    }

    /// After an overflow check at a fixed width: the value the program goes on with.
    fn after_overflow(&mut self, site: Option<SiteId>, ty: &Ty, r: ISet) -> ISet {
        let Some(w) = self.width_of(ty) else {
            if let Some(site) = site {
                self.site(site, true, false, format!("free name, reaches {}", r.describe()));
            }
            return r;
        };
        let Some(site) = site else { return r.clip(w.min(), w.max()) };
        if !self.prog.sites[site as usize].active {
            return r;
        }
        let within = r.within(w.min(), w.max());
        let certain = r.disjoint(w.min(), w.max()) && !r.is_empty();
        self.sites[site as usize].seen = self.sites[site as usize].seen.join(&r);
        let seen = self.sites[site as usize].seen.clone();
        let note = if within { format!("result within {}", seen.describe()) } else { format!("result {} reaches past {}", seen.describe(), w.name()) };
        self.site(site, within, certain, note);
        if within {
            self.sites[site as usize].note = format!("result within {}", seen.describe());
        }
        if within {
            r
        } else if self.tier(site) == Tier::Nocheck {
            ISet::of_width(w)
        } else {
            r.clip(w.min(), w.max())
        }
    }

    fn bounds_check(&self, st: &State, refs: &AListRef, idx: &ISet) -> (bool, bool, String) {
        let mut len = ISet::Empty;
        for id in refs.0.iter() {
            len = len.join(&st.heap[*id as usize].len);
        }
        if refs.0.is_empty() || idx.is_empty() {
            return (true, false, "unreached".into());
        }
        let min_len = len.lo();
        let max_len = len.hi();
        let proven = idx.within(0, min_len.saturating_sub(1));
        let certain = idx.disjoint(0, max_len.saturating_sub(1));
        let note = format!("index {} vs length {}", idx.describe(), len.describe());
        (proven, certain, note)
    }

    pub fn eval(&mut self, st: &mut State, e: &Expr) -> AVal {
        self.tick();
        let v = self.eval_inner(st, e);
        if v != AVal::Undef {
            self.record(e, &v);
        }
        v
    }

    fn eval_inner(&mut self, st: &mut State, e: &Expr) -> AVal {
        match &e.kind {
            EK::Int(v) => match &e.ty {
                Ty::Bin(BinWidth::B32) => AVal::Bin(ABin::Known(*v as f32 as f64)),
                Ty::Bin(_) => AVal::Bin(ABin::Known(*v as f64)),
                _ => AVal::Int(ISet::one(*v)),
            },
            EK::Bin(f) => match &e.ty {
                Ty::Bin(BinWidth::B32) => AVal::Bin(ABin::Known(*f as f32 as f64)),
                _ => AVal::Bin(ABin::Known(*f)),
            },
            EK::Text(s) => AVal::Str(AStr::known(s.clone())),
            EK::Bool(b) => AVal::Bool(BSet::of(*b)),
            EK::Var(v) => {
                let val = st.vars[*v as usize].clone();
                if val == AVal::Undef {
                    // Declared on a path we did not take: anything of its type.
                    top_of(&e.ty)
                } else {
                    val
                }
            }
            EK::Pieces(ps) => {
                let mut known = Some(String::new());
                let mut len = ISet::one(0);
                for p in ps {
                    let (k, l): (Option<String>, ISet) = match p {
                        PieceIr::Text(s) => (Some(s.clone()), ISet::one(s.chars().count() as i128)),
                        PieceIr::Newline => (Some("\n".into()), ISet::one(1)),
                        PieceIr::Var(v, t) => {
                            let val = st.vars[*v as usize].clone();
                            let val = if val == AVal::Undef { top_of(t) } else { val };
                            self.render_known(&val, t, false)
                        }
                        PieceIr::Value(x) => {
                            let val = self.eval(st, x);
                            if val == AVal::Undef {
                                return AVal::Undef;
                            }
                            self.render_known(&val, &x.ty, false)
                        }
                    };
                    known = match (known, k) {
                        (Some(mut a), Some(b)) => { a.push_str(&b); Some(a) }
                        _ => None,
                    };
                    len = len.binop(BinOp::Add, &l);
                }
                AVal::Str(AStr { known, len })
            }
            EK::List(items) => {
                let mut vals = Vec::new();
                for i in items {
                    let v = self.eval(st, i);
                    if v == AVal::Undef {
                        return AVal::Undef;
                    }
                    vals.push(v);
                }
                let elem = vals.iter().fold(AVal::Undef, |a, b| a.join(b));
                self.allocate(st, e.id, AList { len: ISet::one(vals.len() as i128), elem, exact: Some(vals), single: true })
            }
            EK::Neg(x, site) => {
                let v = self.eval(st, x);
                if v == AVal::Undef {
                    return AVal::Undef;
                }
                match v {
                    AVal::Int(s) => {
                        let r = s.neg();
                        AVal::Int(self.after_overflow(*site, &e.ty, r))
                    }
                    AVal::Bin(ABin::Known(f)) => AVal::Bin(ABin::Known(-f)),
                    AVal::Bin(ABin::Unknown) => AVal::Bin(ABin::Unknown),
                    _ => unreachable!(),
                }
            }
            EK::Not(x) => {
                let v = self.eval(st, x);
                if v == AVal::Undef {
                    return AVal::Undef;
                }
                AVal::Bool(v.as_bool().not())
            }
            EK::Binary(op, a, b, s1, s2) => self.binary(st, e, *op, a, b, *s1, *s2),
            EK::Index(l, i, site) => {
                let lv = self.eval(st, l);
                let iv = self.eval(st, i);
                if lv == AVal::Undef || iv == AVal::Undef {
                    return AVal::Undef;
                }
                let AVal::List(refs) = &lv else { unreachable!() };
                let iset = iv.as_int().clone();
                let (proven, certain, note) = self.bounds_check(st, refs, &iset);
                self.site(*site, proven, certain, note);
                if certain {
                    return AVal::Undef;
                }
                let mut out = AVal::Undef;
                for id in refs.0.iter() {
                    let obj = &st.heap[*id as usize];
                    let v = match (&obj.exact, iset.iter_values()) {
                        (Some(ex), Some(vals)) => {
                            let mut acc = AVal::Undef;
                            for k in vals {
                                if k >= 0 && (k as usize) < ex.len() {
                                    acc = acc.join(&ex[k as usize]);
                                }
                            }
                            acc
                        }
                        _ => obj.elem.clone(),
                    };
                    out = out.join(&v);
                }
                if out == AVal::Undef { top_of(&e.ty) } else { out }
            }
            EK::Call(f, args) => {
                let mut vals = Vec::new();
                for a in args {
                    let v = self.eval(st, a);
                    if v == AVal::Undef {
                        return AVal::Undef;
                    }
                    vals.push(v);
                }
                self.call(st, *f, vals)
            }
            EK::Read(_, bounds) => match &e.ty {
                Ty::Int(IntTy::Fixed(w)) => AVal::Int(match bounds.value {
                    Some((lo, hi)) => ISet::range(lo, hi),
                    None => ISet::of_width(*w),
                }),
                Ty::Bin(_) => AVal::Bin(ABin::Unknown),
                Ty::Bool => AVal::Bool(BSet::BOTH),
                Ty::Str => AVal::Str(AStr { known: None, len: match bounds.len { Some((lo, hi)) => ISet::range(lo, hi), None => ISet::Range(0, POS_INF) } }),
                Ty::List(elem) => {
                    let Ty::Int(IntTy::Fixed(w)) = **elem else { unreachable!() };
                    let ev = match bounds.value { Some((lo, hi)) => ISet::range(lo, hi), None => ISet::of_width(w) };
                    let len = match bounds.len { Some((lo, hi)) => ISet::range(lo, hi), None => ISet::Range(0, POS_INF) };
                    self.allocate(st, e.id, AList { len, elem: AVal::Int(ev), exact: None, single: true })
                }
                _ => unreachable!(),
            },
            EK::Len(x) => {
                let v = self.eval(st, x);
                match v {
                    AVal::Undef => AVal::Undef,
                    AVal::Str(s) => AVal::Int(s.len),
                    AVal::List(refs) => {
                        let mut len = ISet::Empty;
                        for id in refs.0.iter() {
                            len = len.join(&st.heap[*id as usize].len);
                        }
                        AVal::Int(if len.is_empty() { ISet::Range(0, POS_INF) } else { len })
                    }
                    _ => unreachable!(),
                }
            }
            EK::Fill(v, n, site) => {
                let val = self.eval(st, v);
                let nv = self.eval(st, n);
                if val == AVal::Undef || nv == AVal::Undef {
                    return AVal::Undef;
                }
                let ns = nv.as_int().clone();
                let proven = ns.lo() >= 0;
                let certain = ns.hi() < 0;
                self.site(*site, proven, certain, format!("count {}", ns.describe()));
                if certain {
                    return AVal::Undef;
                }
                let len = ns.clip(0, POS_INF);
                let exact = match len.single() {
                    Some(k) if k <= SET_CAP as i128 => Some(vec![val.clone(); k as usize]),
                    _ => None,
                };
                self.allocate(st, e.id, AList { len, elem: val, exact, single: true })
            }
            EK::To(x, site) => {
                let v = self.eval(st, x);
                if v == AVal::Undef {
                    return AVal::Undef;
                }
                match (&v, &e.ty) {
                    (AVal::Int(s), Ty::Int(_)) => AVal::Int(self.after_overflow(*site, &e.ty, s.clone())),
                    (AVal::Int(s), Ty::Bin(w)) => AVal::Bin(match s.single() {
                        Some(k) => ABin::Known(match w { BinWidth::B32 => k as f32 as f64, BinWidth::B64 => k as f64 }),
                        None => ABin::Unknown,
                    }),
                    (AVal::Bin(b), Ty::Int(_)) => {
                        let w = self.width_of(&e.ty).unwrap_or(IntWidth::I64);
                        match b {
                            ABin::Known(f) => {
                                let t = f.trunc();
                                let fits = t.is_finite() && t >= w.min() as f64 && t <= w.max() as f64;
                                if let Some(site) = site {
                                    self.site(*site, fits, !fits, format!("value {}", render::useful64(*f)));
                                }
                                if fits { AVal::Int(ISet::one(t as i128)) } else { AVal::Int(ISet::of_width(w)) }
                            }
                            ABin::Unknown => {
                                if let Some(site) = site {
                                    self.site(*site, false, false, "any bin".into());
                                }
                                AVal::Int(ISet::of_width(w))
                            }
                        }
                    }
                    (AVal::Bin(b), Ty::Bin(w)) => AVal::Bin(match b {
                        ABin::Known(f) => ABin::Known(match w { BinWidth::B32 => *f as f32 as f64, BinWidth::B64 => *f }),
                        ABin::Unknown => ABin::Unknown,
                    }),
                    (_, Ty::Str) => {
                        let (k, l) = self.render_known(&v, &x.ty, true);
                        AVal::Str(AStr { known: k, len: l })
                    }
                    _ => v,
                }
            }
        }
    }

    fn allocate(&mut self, st: &mut State, expr_id: u32, obj: AList) -> AVal {
        let id = match self.alloc.get(&expr_id) {
            Some(id) if (*id as usize) < st.heap.len() => {
                // The same site allocating again: one object for both; strong updates end.
                let id = *id;
                let old = st.heap[id as usize].clone();
                let mut merged = old.join(&obj);
                merged.single = false;
                st.heap[id as usize] = merged;
                id
            }
            Some(id) if (*id as usize) == st.heap.len() => {
                let id = *id;
                st.heap.push(obj);
                id
            }
            Some(id) => {
                // The heap is shorter than the id: this path never allocated the
                // sites in between; pad so ids stay stable across joins.
                let id = *id;
                while st.heap.len() < id as usize {
                    st.heap.push(AList { len: ISet::Empty, elem: AVal::Undef, exact: None, single: true });
                }
                st.heap.push(obj);
                id
            }
            None => {
                let id = self.alloc.len() as u32;
                self.alloc.insert(expr_id, id);
                while st.heap.len() < id as usize {
                    st.heap.push(AList { len: ISet::Empty, elem: AVal::Undef, exact: None, single: true });
                }
                st.heap.push(obj);
                id
            }
        };
        AVal::List(AListRef(BTreeSet::from([id])))
    }

    fn render_known(&self, v: &AVal, ty: &Ty, useful: bool) -> (Option<String>, ISet) {
        match v {
            AVal::Int(s) => match s.single() {
                Some(k) => (Some(k.to_string()), ISet::one(k.to_string().len() as i128)),
                None => (None, ISet::Range(1, 41)),
            },
            AVal::Bin(ABin::Known(f)) => {
                let s = match (ty, useful) {
                    (Ty::Bin(BinWidth::B32), false) => render::stored32(*f as f32),
                    (Ty::Bin(BinWidth::B32), true) => render::useful32(*f as f32),
                    (_, false) => render::stored64(*f),
                    (_, true) => render::useful64(*f),
                };
                let n = s.len() as i128;
                (Some(s), ISet::one(n))
            }
            AVal::Bin(ABin::Unknown) => (None, ISet::Range(1, 1100)),
            AVal::Bool(b) => match b.single() {
                Some(t) => (Some(t.to_string()), ISet::one(if t { 4 } else { 5 })),
                None => (None, ISet::range(4, 5)),
            },
            AVal::Str(s) => (s.known.clone(), s.len.clone()),
            AVal::List(_) => (None, ISet::Range(2, POS_INF)),
            _ => (None, ISet::Range(0, POS_INF)),
        }
    }

    fn binary(&mut self, st: &mut State, e: &Expr, op: BinOp, a: &Expr, b: &Expr, s1: Option<SiteId>, s2: Option<SiteId>) -> AVal {
        match op {
            BinOp::And | BinOp::Or => {
                let av = self.eval(st, a);
                if av == AVal::Undef {
                    return AVal::Undef;
                }
                let ab = av.as_bool();
                let short = if op == BinOp::And { !ab.can_true() } else { !ab.can_false() };
                if short {
                    return AVal::Bool(ab);
                }
                // b is evaluated only where a did not decide; narrow a's effect first.
                let mut sb = self.narrow(st, a, op == BinOp::And);
                let bv = self.eval(&mut sb, b);
                if bv == AVal::Undef {
                    return AVal::Undef;
                }
                st.heap = sb.heap;
                let bb = bv.as_bool();
                let r = if op == BinOp::And {
                    BSet::new(ab.can_true() && bb.can_true(), ab.can_false() || bb.can_false())
                } else {
                    BSet::new(ab.can_true() || bb.can_true(), ab.can_false() && bb.can_false())
                };
                AVal::Bool(r)
            }
            _ => {
                let av = self.eval(st, a);
                let bv = self.eval(st, b);
                if av == AVal::Undef || bv == AVal::Undef {
                    return AVal::Undef;
                }
                if op.is_cmp() {
                    return AVal::Bool(match (&av, &bv) {
                        (AVal::Int(x), AVal::Int(y)) => x.cmp(op, y),
                        (AVal::Bin(ABin::Known(x)), AVal::Bin(ABin::Known(y))) => {
                            let (x, y) = match &a.ty { Ty::Bin(BinWidth::B32) => (*x as f32 as f64, *y as f32 as f64), _ => (*x, *y) };
                            BSet::of(match op {
                                BinOp::Eq => x == y, BinOp::Ne => x != y, BinOp::Lt => x < y, BinOp::Le => x <= y,
                                BinOp::Gt => x > y, BinOp::Ge => x >= y, _ => unreachable!(),
                            })
                        }
                        (AVal::Bin(_), AVal::Bin(_)) => BSet::BOTH,
                        (AVal::Bool(x), AVal::Bool(y)) => match (x.single(), y.single()) {
                            (Some(p), Some(q)) => BSet::of(if op == BinOp::Eq { p == q } else { p != q }),
                            _ => BSet::BOTH,
                        },
                        (AVal::Str(x), AVal::Str(y)) => match (&x.known, &y.known) {
                            (Some(p), Some(q)) => BSet::of(if op == BinOp::Eq { p == q } else { p != q }),
                            _ => {
                                // Different lengths cannot be equal.
                                let same_len_possible = !(x.len.hi() < y.len.lo() || x.len.lo() > y.len.hi());
                                if same_len_possible { BSet::BOTH } else { BSet::of(op == BinOp::Ne) }
                            }
                        },
                        _ => BSet::BOTH,
                    });
                }
                match (&av, &bv) {
                    (AVal::Int(x), AVal::Int(y)) => {
                        let mut y = y.clone();
                        // Secondary sites: zero divisor, negative exponent.
                        match op {
                            BinOp::Div | BinOp::Mod => {
                                if let Some(site) = s2 {
                                    let has_zero = y.contains(0);
                                    let only_zero = y.single() == Some(0);
                                    self.site(site, !has_zero, only_zero, format!("divisor {}", y.describe()));
                                    if only_zero {
                                        return AVal::Undef;
                                    }
                                }
                                y = y.without(0);
                            }
                            BinOp::Pow => {
                                if let Some(site) = s2 {
                                    let neg = y.lo() < 0;
                                    let only_neg = y.hi() < 0;
                                    self.site(site, !neg, only_neg, format!("exponent {}", y.describe()));
                                    if only_neg {
                                        return AVal::Undef;
                                    }
                                }
                                y = y.clip(0, POS_INF);
                            }
                            _ => {}
                        }
                        let r = x.binop(op, &y);
                        AVal::Int(self.after_overflow(s1, &e.ty, r))
                    }
                    (AVal::Bin(x), AVal::Bin(y)) => AVal::Bin(match (x, y) {
                        (ABin::Known(x), ABin::Known(y)) => {
                            let r = match &e.ty {
                                Ty::Bin(BinWidth::B32) => {
                                    let (x, y) = (*x as f32, *y as f32);
                                    (match op { BinOp::Add => x + y, BinOp::Sub => x - y, BinOp::Mul => x * y, BinOp::Div => x / y, BinOp::Pow => x.powf(y), _ => unreachable!() }) as f64
                                }
                                _ => match op { BinOp::Add => x + y, BinOp::Sub => x - y, BinOp::Mul => x * y, BinOp::Div => x / y, BinOp::Pow => x.powf(*y), _ => unreachable!() },
                            };
                            ABin::Known(r)
                        }
                        _ => ABin::Unknown,
                    }),
                    _ => unreachable!("arith on {av:?} {bv:?}"),
                }
            }
        }
    }
}

fn widen_val(old: &AVal, new: &AVal) -> AVal {
    match (old, new) {
        (AVal::Int(a), AVal::Int(b)) => AVal::Int(a.widen(b, NEG_INF, POS_INF)),
        (AVal::Str(a), AVal::Str(b)) => AVal::Str(AStr { known: if a.known == b.known { a.known.clone() } else { None }, len: a.len.widen(&b.len, 0, POS_INF) }),
        (_, b) => b.clone(),
    }
}

fn collect_calls(stmts: &[Stmt], out: &mut Vec<FuncId>) {
    fn expr(e: &Expr, out: &mut Vec<FuncId>) {
        match &e.kind {
            EK::Call(f, args) => {
                out.push(*f);
                args.iter().for_each(|a| expr(a, out));
            }
            EK::Pieces(ps) => ps.iter().for_each(|p| if let PieceIr::Value(v) = p { expr(v, out) }),
            EK::List(es) => es.iter().for_each(|a| expr(a, out)),
            EK::Neg(x, _) | EK::Not(x) | EK::Len(x) | EK::To(x, _) => expr(x, out),
            EK::Binary(_, a, b, ..) | EK::Index(a, b, _) | EK::Fill(a, b, _) => { expr(a, out); expr(b, out); }
            _ => {}
        }
    }
    for s in stmts {
        match &s.kind {
            SK::Let(_, e) | SK::Assign(_, e) | SK::Exit(e) | SK::CallStmt(e) | SK::Return(Some(e)) => expr(e, out),
            SK::AssignIndex(t, i, v, _) => { expr(t, out); expr(i, out); expr(v, out); }
            SK::Print { pieces, .. } => pieces.iter().for_each(|p| expr(p, out)),
            SK::If(brs, el) => {
                for (c, b) in brs { expr(c, out); collect_calls(b, out); }
                if let Some(b) = el { collect_calls(b, out); }
            }
            SK::Block(b) | SK::Loop(b) => collect_calls(b, out),
            SK::While(c, b) => { expr(c, out); collect_calls(b, out); }
            SK::ForRange { a, b, body, .. } => { expr(a, out); expr(b, out); collect_calls(body, out); }
            SK::ForList { list, body, .. } => { expr(list, out); collect_calls(body, out); }
            _ => {}
        }
    }
}

fn collect_exprs(stmts: &[Stmt], out: &mut Vec<u32>) {
    fn expr(e: &Expr, out: &mut Vec<u32>) {
        out.push(e.id);
        match &e.kind {
            EK::Call(_, args) | EK::List(args) => args.iter().for_each(|a| expr(a, out)),
            EK::Pieces(ps) => ps.iter().for_each(|p| if let PieceIr::Value(v) = p { expr(v, out) }),
            EK::Neg(x, _) | EK::Not(x) | EK::Len(x) | EK::To(x, _) => expr(x, out),
            EK::Binary(_, a, b, ..) | EK::Index(a, b, _) | EK::Fill(a, b, _) => { expr(a, out); expr(b, out); }
            _ => {}
        }
    }
    for s in stmts {
        match &s.kind {
            SK::Let(_, e) | SK::Assign(_, e) | SK::Exit(e) | SK::CallStmt(e) | SK::Return(Some(e)) => expr(e, out),
            SK::AssignIndex(t, i, v, _) => { expr(t, out); expr(i, out); expr(v, out); }
            SK::Print { pieces, .. } => pieces.iter().for_each(|p| expr(p, out)),
            SK::If(brs, el) => {
                for (c, b) in brs { expr(c, out); collect_exprs(b, out); }
                if let Some(b) = el { collect_exprs(b, out); }
            }
            SK::Block(b) | SK::Loop(b) => collect_exprs(b, out),
            SK::While(c, b) => { expr(c, out); collect_exprs(b, out); }
            SK::ForRange { a, b, body, .. } => { expr(a, out); expr(b, out); collect_exprs(body, out); }
            SK::ForList { list, body, .. } => { expr(list, out); collect_exprs(body, out); }
            _ => {}
        }
    }
}
