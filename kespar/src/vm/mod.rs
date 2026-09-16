//! The bytecode interpreter: values, a mark-sweep heap, and the run loop.

pub mod bytecode;
pub mod render;

use std::io::{BufRead, Write};

use crate::ast::{BinOp, BinWidth, IntWidth};
use crate::ir::{ReadBounds, SiteId, SiteKind};
use crate::types::{IntTy, Ty};
use bytecode::*;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    Int(i128),
    Bin64(f64),
    Bin32(f32),
    Bool(bool),
    /// Index into the heap.
    Str(u32),
    List(u32),
    Nothing,
}

#[derive(Debug, Clone)]
pub enum Object {
    Str(String),
    List(Vec<Value>),
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// `MAIN` ended or `std::exit[n]`.
    Exit(u8),
    /// A check failed: exit 1.
    Trap { kind: SiteKind, line: u32, site: Option<SiteId> },
    /// The read contract was violated: exit 2.
    BadInput { name: String, line: u32 },
    /// The step budget ran out: exit 4.
    Budget,
}

impl Outcome {
    pub fn exit_code(&self) -> i32 {
        match self {
            Outcome::Exit(n) => *n as i32,
            Outcome::Trap { .. } => 1,
            Outcome::BadInput { .. } => 2,
            Outcome::Budget => 4,
        }
    }
    pub fn stderr_line(&self) -> Option<String> {
        match self {
            Outcome::Exit(_) => None,
            Outcome::Trap { kind, line, .. } => Some(format!("kespar: {} at line {line}", kind.text())),
            Outcome::BadInput { name, line } => Some(format!("kespar: bad input for '{name}' at line {line}")),
            Outcome::Budget => Some("kespar: step budget exhausted".into()),
        }
    }
}

struct Frame {
    func: u32,
    pc: usize,
    base: usize,
}

pub struct Vm<'a> {
    module: &'a Module,
    stack: Vec<Value>,
    locals: Vec<Value>,
    frames: Vec<Frame>,
    heap: Vec<Option<Object>>,
    free: Vec<u32>,
    gc_threshold: usize,
    pub out: Vec<u8>,
    input: Box<dyn BufRead + 'a>,
    /// Steps left, or `None` for no budget.
    budget: Option<u64>,
    pub steps: u64,
    /// Which sites fired (for the oracle's trace).
    pub fired: Vec<(SiteId, u32)>,
}

pub struct RunResult {
    pub outcome: Outcome,
    pub stdout: Vec<u8>,
    pub steps: u64,
}

pub fn run(module: &Module, input: Box<dyn BufRead + '_>, budget: Option<u64>) -> RunResult {
    let mut vm = Vm { module, stack: Vec::new(), locals: Vec::new(), frames: Vec::new(), heap: Vec::new(), free: Vec::new(), gc_threshold: 1 << 16, out: Vec::new(), input, budget, steps: 0, fired: Vec::new() };
    let outcome = vm.run_main();
    RunResult { outcome, stdout: vm.out, steps: vm.steps }
}

/// Run and write stdout/stderr like a real program would; returns the exit code.
pub fn run_process(module: &Module, budget: Option<u64>) -> i32 {
    let stdin = std::io::stdin();
    let r = run(module, Box::new(stdin.lock()), budget);
    let mut so = std::io::stdout().lock();
    let _ = so.write_all(&r.stdout);
    let _ = so.flush();
    if let Some(l) = r.outcome.stderr_line() {
        eprintln!("{l}");
    }
    r.outcome.exit_code()
}

enum Flow {
    Continue,
    Done(Outcome),
}

impl<'a> Vm<'a> {
    fn run_main(&mut self) -> Outcome {
        let main = &self.module.funcs[self.module.main as usize];
        self.locals.resize(main.nlocals as usize, Value::Nothing);
        self.frames.push(Frame { func: self.module.main, pc: 0, base: 0 });
        loop {
            match self.step() {
                Ok(Flow::Continue) => {}
                Ok(Flow::Done(o)) => return o,
                Err(o) => return o,
            }
        }
    }

    fn alloc(&mut self, o: Object) -> u32 {
        if self.heap.len() >= self.gc_threshold && self.free.is_empty() {
            self.collect();
            if self.free.len() < self.heap.len() / 2 {
                self.gc_threshold *= 2;
            }
        }
        if let Some(i) = self.free.pop() {
            self.heap[i as usize] = Some(o);
            i
        } else {
            self.heap.push(Some(o));
            (self.heap.len() - 1) as u32
        }
    }

    fn collect(&mut self) {
        let mut marked = vec![false; self.heap.len()];
        let mut work: Vec<u32> = Vec::new();
        for v in self.stack.iter().chain(self.locals.iter()) {
            if let Value::Str(i) | Value::List(i) = v {
                work.push(*i);
            }
        }
        while let Some(i) = work.pop() {
            if marked[i as usize] {
                continue;
            }
            marked[i as usize] = true;
            if let Some(Object::List(vs)) = &self.heap[i as usize] {
                for v in vs {
                    if let Value::Str(j) | Value::List(j) = v {
                        if !marked[*j as usize] {
                            work.push(*j);
                        }
                    }
                }
            }
        }
        self.free.clear();
        for (i, m) in marked.iter().enumerate() {
            if !m && self.heap[i].is_some() {
                self.heap[i] = None;
                self.free.push(i as u32);
            } else if !m {
                self.free.push(i as u32);
            }
        }
    }

    fn str_of(&self, i: u32) -> &str {
        match &self.heap[i as usize] {
            Some(Object::Str(s)) => s,
            _ => unreachable!("not a str"),
        }
    }

    fn list_of(&self, i: u32) -> &Vec<Value> {
        match &self.heap[i as usize] {
            Some(Object::List(v)) => v,
            _ => unreachable!("not a list"),
        }
    }

    fn push_str(&mut self, s: String) {
        let i = self.alloc(Object::Str(s));
        self.stack.push(Value::Str(i));
    }

    fn pop(&mut self) -> Value {
        self.stack.pop().expect("stack underflow")
    }

    fn pop_int(&mut self) -> i128 {
        match self.pop() {
            Value::Int(v) => v,
            other => unreachable!("expected int, got {other:?}"),
        }
    }

    fn pop_bool(&mut self) -> bool {
        match self.pop() {
            Value::Bool(b) => b,
            other => unreachable!("expected bool, got {other:?}"),
        }
    }

    fn trap(&mut self, kind: SiteKind, line: u32, site: Option<SiteId>) -> Outcome {
        if let Some(s) = site {
            self.fired.push((s, line));
        }
        Outcome::Trap { kind, line, site }
    }

    pub fn render(&self, v: Value, useful: bool) -> String {
        match v {
            Value::Int(i) => i.to_string(),
            Value::Bin64(f) => if useful { render::useful64(f) } else { render::stored64(f) },
            Value::Bin32(f) => if useful { render::useful32(f) } else { render::stored32(f) },
            Value::Bool(b) => b.to_string(),
            Value::Str(i) => self.str_of(i).to_string(),
            Value::List(i) => {
                let items: Vec<String> = self.list_of(i).iter().map(|x| self.render(*x, useful)).collect();
                format!("[{}]", items.join(", "))
            }
            Value::Nothing => String::new(),
        }
    }

    fn step(&mut self) -> Result<Flow, Outcome> {
        if let Some(b) = self.budget {
            if b == 0 {
                return Err(Outcome::Budget);
            }
            self.budget = Some(b - 1);
        }
        self.steps += 1;
        let fi = self.frames.last().unwrap().func as usize;
        let pc = self.frames.last().unwrap().pc;
        let base = self.frames.last().unwrap().base;
        let op = &self.module.funcs[fi].ops[pc];
        self.frames.last_mut().unwrap().pc += 1;
        match op {
            Op::Const(i) => {
                let v = match &self.module.consts[*i as usize] {
                    Const::Int(v) => Value::Int(*v),
                    Const::Bin64(f) => Value::Bin64(*f),
                    Const::Bin32(f) => Value::Bin32(*f),
                    Const::Bool(b) => Value::Bool(*b),
                    Const::Str(s) => {
                        let s = s.clone();
                        self.push_str(s);
                        return Ok(Flow::Continue);
                    }
                };
                self.stack.push(v);
            }
            Op::Load(i) => {
                let v = self.locals[base + *i as usize];
                self.stack.push(v);
            }
            Op::Store(i) => {
                let v = self.pop();
                self.locals[base + *i as usize] = v;
            }
            Op::Pop => {
                self.pop();
            }
            Op::IntOp { op, width, overflow, second, line } => {
                let (op, width, overflow, second, line) = (*op, *width, *overflow, *second, *line);
                let b = self.pop_int();
                let a = self.pop_int();
                let r = match op {
                    BinOp::Add => a + b,
                    BinOp::Sub => a - b,
                    BinOp::Mul => match a.checked_mul(b) {
                        Some(v) => v,
                        None => {
                            if overflow.is_some() {
                                return Err(self.trap(SiteKind::Overflow, line, overflow));
                            }
                            width.wrap(a.wrapping_mul(b))
                        }
                    },
                    BinOp::Div | BinOp::Mod => {
                        if b == 0 {
                            if second.is_some() {
                                return Err(self.trap(SiteKind::DivZero, line, second));
                            }
                            // nocheck: unspecified; we stop anyway
                            return Err(self.trap(SiteKind::DivZero, line, None));
                        }
                        if op == BinOp::Div { a / b } else { a % b }
                    }
                    BinOp::Pow => {
                        if b < 0 {
                            if second.is_some() {
                                return Err(self.trap(SiteKind::NegExp, line, second));
                            }
                            return Err(self.trap(SiteKind::NegExp, line, None));
                        }
                        match int_pow(a, b as u128, width) {
                            Some(v) => v,
                            None => {
                                if overflow.is_some() {
                                    return Err(self.trap(SiteKind::Overflow, line, overflow));
                                }
                                wrapping_pow(a, b as u128, width)
                            }
                        }
                    }
                    _ => unreachable!(),
                };
                let r = if width.holds(r) {
                    r
                } else if overflow.is_some() {
                    return Err(self.trap(SiteKind::Overflow, line, overflow));
                } else {
                    width.wrap(r)
                };
                self.stack.push(Value::Int(r));
            }
            Op::IntNeg { width, overflow, line } => {
                let (width, overflow, line) = (*width, *overflow, *line);
                let a = self.pop_int();
                let r = -a;
                let r = if width.holds(r) {
                    r
                } else if overflow.is_some() {
                    return Err(self.trap(SiteKind::Overflow, line, overflow));
                } else {
                    width.wrap(r)
                };
                self.stack.push(Value::Int(r));
            }
            Op::BinOp { op, width } => {
                let (op, width) = (*op, *width);
                let b = self.pop();
                let a = self.pop();
                let v = match (width, a, b) {
                    (BinWidth::B64, Value::Bin64(a), Value::Bin64(b)) => Value::Bin64(match op {
                        BinOp::Add => a + b, BinOp::Sub => a - b, BinOp::Mul => a * b, BinOp::Div => a / b,
                        BinOp::Pow => a.powf(b), _ => unreachable!(),
                    }),
                    (BinWidth::B32, Value::Bin32(a), Value::Bin32(b)) => Value::Bin32(match op {
                        BinOp::Add => a + b, BinOp::Sub => a - b, BinOp::Mul => a * b, BinOp::Div => a / b,
                        BinOp::Pow => a.powf(b), _ => unreachable!(),
                    }),
                    other => unreachable!("bin op on {other:?}"),
                };
                self.stack.push(v);
            }
            Op::BinNeg { .. } => {
                let v = match self.pop() {
                    Value::Bin64(f) => Value::Bin64(-f),
                    Value::Bin32(f) => Value::Bin32(-f),
                    _ => unreachable!(),
                };
                self.stack.push(v);
            }
            Op::Cmp { op, kind } => {
                let (op, kind) = (*op, *kind);
                let b = self.pop();
                let a = self.pop();
                let r = match kind {
                    CmpKind::Int => {
                        let (Value::Int(a), Value::Int(b)) = (a, b) else { unreachable!() };
                        cmp_ord(op, a.cmp(&b))
                    }
                    CmpKind::Bin64 => {
                        let (Value::Bin64(a), Value::Bin64(b)) = (a, b) else { unreachable!() };
                        cmp_float(op, a.partial_cmp(&b))
                    }
                    CmpKind::Bin32 => {
                        let (Value::Bin32(a), Value::Bin32(b)) = (a, b) else { unreachable!() };
                        cmp_float(op, a.partial_cmp(&b))
                    }
                    CmpKind::Bool => {
                        let (Value::Bool(a), Value::Bool(b)) = (a, b) else { unreachable!() };
                        match op { BinOp::Eq => a == b, BinOp::Ne => a != b, _ => unreachable!() }
                    }
                    CmpKind::Str => {
                        let (Value::Str(a), Value::Str(b)) = (a, b) else { unreachable!() };
                        let eq = self.str_of(a) == self.str_of(b);
                        match op { BinOp::Eq => eq, BinOp::Ne => !eq, _ => unreachable!() }
                    }
                };
                self.stack.push(Value::Bool(r));
            }
            Op::Not => {
                let b = self.pop_bool();
                self.stack.push(Value::Bool(!b));
            }
            Op::Jump(t) => self.frames.last_mut().unwrap().pc = *t as usize,
            Op::JumpIfFalse(t) => {
                let t = *t;
                if !self.pop_bool() {
                    self.frames.last_mut().unwrap().pc = t as usize;
                }
            }
            Op::JumpIfTrue(t) => {
                let t = *t;
                if self.pop_bool() {
                    self.frames.last_mut().unwrap().pc = t as usize;
                }
            }
            Op::NewList(n) => {
                let n = *n as usize;
                let items = self.stack.split_off(self.stack.len() - n);
                let i = self.alloc(Object::List(items));
                self.stack.push(Value::List(i));
            }
            Op::Fill { site, line } => {
                let (site, line) = (*site, *line);
                let n = self.pop_int();
                let v = self.pop();
                if n < 0 {
                    return Err(self.trap(SiteKind::OutOfBounds, line, site));
                }
                if n > (1 << 31) {
                    // Far past anything this VM can hold; treat as out of bounds rather than aborting.
                    return Err(self.trap(SiteKind::OutOfBounds, line, site));
                }
                let i = self.alloc(Object::List(vec![v; n as usize]));
                self.stack.push(Value::List(i));
            }
            Op::Index { site, line } => {
                let (site, line) = (*site, *line);
                let idx = self.pop_int();
                let Value::List(l) = self.pop() else { unreachable!() };
                let list = self.list_of(l);
                if idx < 0 || idx as usize >= list.len() {
                    return Err(self.trap(SiteKind::OutOfBounds, line, site));
                }
                let v = list[idx as usize];
                self.stack.push(v);
            }
            Op::StoreIndex { site, line } => {
                let (site, line) = (*site, *line);
                let v = self.pop();
                let idx = self.pop_int();
                let Value::List(l) = self.pop() else { unreachable!() };
                let len = self.list_of(l).len();
                if idx < 0 || idx as usize >= len {
                    return Err(self.trap(SiteKind::OutOfBounds, line, site));
                }
                if let Some(Object::List(items)) = &mut self.heap[l as usize] {
                    items[idx as usize] = v;
                }
            }
            Op::Len => {
                let n = match self.pop() {
                    Value::List(l) => self.list_of(l).len(),
                    Value::Str(s) => self.str_of(s).chars().count(),
                    _ => unreachable!(),
                };
                self.stack.push(Value::Int(n as i128));
            }
            Op::Render { useful, ty: _ } => {
                let useful = *useful;
                let v = self.pop();
                let s = self.render(v, useful);
                self.push_str(s);
            }
            Op::Concat(n) => {
                let n = *n as usize;
                let parts = self.stack.split_off(self.stack.len() - n);
                let mut s = String::new();
                for p in parts {
                    let Value::Str(i) = p else { unreachable!() };
                    s.push_str(self.str_of(i));
                }
                self.push_str(s);
            }
            Op::Print => {
                let Value::Str(i) = self.pop() else { unreachable!() };
                let s = self.str_of(i).to_string();
                self.out.extend_from_slice(s.as_bytes());
            }
            Op::Read { read, ty, bounds, line } => {
                let (read, ty, bounds, line) = (*read, ty.clone(), bounds.clone(), *line);
                let name = self.module.read_names[read as usize].clone();
                let mut buf = String::new();
                let got = self.input.read_line(&mut buf).unwrap_or(0);
                if got == 0 {
                    return Err(Outcome::BadInput { name, line });
                }
                let line_str = buf.strip_suffix('\n').unwrap_or(&buf);
                let line_str = line_str.strip_suffix('\r').unwrap_or(line_str);
                match self.parse_read(&ty, &bounds, line_str) {
                    Some(v) => self.stack.push(v),
                    None => return Err(Outcome::BadInput { name, line }),
                }
            }
            Op::Call(f) => {
                let f = *f;
                let code = &self.module.funcs[f as usize];
                let nparams = code.nparams as usize;
                let new_base = self.locals.len();
                self.locals.resize(new_base + code.nlocals as usize, Value::Nothing);
                let args = self.stack.split_off(self.stack.len() - nparams);
                self.locals[new_base..new_base + nparams].copy_from_slice(&args);
                self.frames.push(Frame { func: f, pc: 0, base: new_base });
                if self.frames.len() > 100_000 {
                    // Deep recursion: the VM cannot go on; report as a budget-style stop.
                    return Err(Outcome::Budget);
                }
            }
            Op::Ret => {
                let v = self.pop();
                let fr = self.frames.pop().unwrap();
                self.locals.truncate(fr.base);
                self.stack.push(v);
            }
            Op::RetNothing => {
                let fr = self.frames.pop().unwrap();
                self.locals.truncate(fr.base);
                if self.frames.is_empty() {
                    return Ok(Flow::Done(Outcome::Exit(0)));
                }
            }
            Op::Exit => {
                let n = self.pop_int();
                return Ok(Flow::Done(Outcome::Exit(n as u8)));
            }
            Op::IntToInt { width, overflow, line } => {
                let (width, overflow, line) = (*width, *overflow, *line);
                let v = self.pop_int();
                let r = if width.holds(v) {
                    v
                } else if overflow.is_some() {
                    return Err(self.trap(SiteKind::Overflow, line, overflow));
                } else {
                    width.wrap(v)
                };
                self.stack.push(Value::Int(r));
            }
            Op::IntToBin { width } => {
                let v = self.pop_int();
                self.stack.push(match width { BinWidth::B64 => Value::Bin64(v as f64), BinWidth::B32 => Value::Bin32(v as f32) });
            }
            Op::BinToInt { width, overflow, line } => {
                let (width, overflow, line) = (*width, *overflow, *line);
                let f = match self.pop() { Value::Bin64(f) => f, Value::Bin32(f) => f as f64, _ => unreachable!() };
                let t = f.trunc();
                let ok = t.is_finite() && t >= width.min() as f64 && t <= width.max() as f64;
                let r = if ok {
                    t as i128
                } else if overflow.is_some() {
                    return Err(self.trap(SiteKind::Overflow, line, overflow));
                } else if t.is_finite() {
                    width.wrap(t as i128)
                } else {
                    0
                };
                self.stack.push(Value::Int(r));
            }
            Op::BinToBin { width } => {
                let v = self.pop();
                self.stack.push(match (width, v) {
                    (BinWidth::B64, Value::Bin32(f)) => Value::Bin64(f as f64),
                    (BinWidth::B32, Value::Bin64(f)) => Value::Bin32(f as f32),
                    (_, v) => v,
                });
            }
            Op::Nop => {}
            Op::Halt => return Ok(Flow::Done(Outcome::Exit(0))),
        }
        Ok(Flow::Continue)
    }

    fn parse_read(&mut self, ty: &Ty, bounds: &ReadBounds, line: &str) -> Option<Value> {
        match ty {
            Ty::Int(IntTy::Fixed(w)) => {
                let v = parse_int(line.trim())?;
                if !w.holds(v) { return None; }
                if let Some((lo, hi)) = bounds.value {
                    if v < lo || v > hi { return None; }
                }
                Some(Value::Int(v))
            }
            Ty::Bin(BinWidth::B64) => line.trim().parse::<f64>().ok().map(Value::Bin64),
            Ty::Bin(BinWidth::B32) => line.trim().parse::<f32>().ok().map(Value::Bin32),
            Ty::Bool => match line.trim() { "true" => Some(Value::Bool(true)), "false" => Some(Value::Bool(false)), _ => None },
            Ty::Str => {
                if let Some((lo, hi)) = bounds.len {
                    let n = line.chars().count() as i128;
                    if n < lo || n > hi { return None; }
                }
                let i = self.alloc(Object::Str(line.to_string()));
                Some(Value::Str(i))
            }
            Ty::List(elem) => {
                let Ty::Int(IntTy::Fixed(w)) = **elem else { return None };
                let mut items = Vec::new();
                for tok in line.split_ascii_whitespace() {
                    let v = parse_int(tok)?;
                    if !w.holds(v) { return None; }
                    if let Some((lo, hi)) = bounds.value {
                        if v < lo || v > hi { return None; }
                    }
                    items.push(Value::Int(v));
                }
                if let Some((lo, hi)) = bounds.len {
                    let n = items.len() as i128;
                    if n < lo || n > hi { return None; }
                }
                let i = self.alloc(Object::List(items));
                Some(Value::List(i))
            }
            _ => None,
        }
    }
}

pub fn parse_int(s: &str) -> Option<i128> {
    let (neg, digits) = match s.strip_prefix('-') {
        Some(d) => (true, d),
        None => (false, s),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) || digits.len() > 40 {
        return None;
    }
    let v: i128 = digits.parse().ok()?;
    Some(if neg { -v } else { v })
}

fn cmp_ord(op: BinOp, o: std::cmp::Ordering) -> bool {
    use std::cmp::Ordering::*;
    match op {
        BinOp::Eq => o == Equal, BinOp::Ne => o != Equal, BinOp::Lt => o == Less, BinOp::Gt => o == Greater,
        BinOp::Le => o != Greater, BinOp::Ge => o != Less, _ => unreachable!(),
    }
}

fn cmp_float(op: BinOp, o: Option<std::cmp::Ordering>) -> bool {
    match o {
        Some(o) => cmp_ord(op, o),
        None => op == BinOp::Ne, // NaN
    }
}

/// Exact integer power, `None` if it leaves the width.
pub fn int_pow(a: i128, b: u128, width: IntWidth) -> Option<i128> {
    if b == 0 { return Some(1); }
    match a {
        0 => return Some(0),
        1 => return Some(1),
        -1 => return Some(if b % 2 == 0 { 1 } else { -1 }),
        _ => {}
    }
    let mut r: i128 = 1;
    let mut i = 0u128;
    while i < b {
        r = r.checked_mul(a)?;
        if !width.holds(r) {
            return None;
        }
        i += 1;
    }
    Some(r)
}

/// Two's-complement power at `width` (for a site with no check).
pub fn wrapping_pow(a: i128, mut b: u128, width: IntWidth) -> i128 {
    let mut base = width.wrap(a);
    let mut r: i128 = 1;
    while b > 0 {
        if b & 1 == 1 {
            r = width.wrap(r.wrapping_mul(base));
        }
        base = width.wrap(base.wrapping_mul(base));
        b >>= 1;
    }
    width.wrap(r)
}
