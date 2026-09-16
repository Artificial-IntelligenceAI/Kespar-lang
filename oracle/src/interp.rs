//! The tree-walking interpreter: §4 reads, §5 statements, §6 expressions,
//! §8.4 run-time checks at every site, §9 exit codes, the step budget and
//! the `--trace-checks` bookkeeping.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::*;
use crate::check::{Checked, NodeInfo, RTy, SiteKind};
use crate::render;
use crate::{Options, Outcome};

#[derive(Clone, Debug)]
pub enum Value {
    /// An integer; `None` width means a free (unbounded) integer.
    Int(i128, Option<IntW>),
    Bin32(f32),
    Bin64(f64),
    Bool(bool),
    Str(Rc<str>),
    List(Rc<RefCell<Vec<Value>>>),
    Nothing,
}

/// Why the program stopped before `MAIN` ended.
struct Stop {
    code: i32,
    msg: Option<String>,
}

enum Flow {
    Normal,
    Break,
    Continue,
    Return(Value),
}

type R<T> = Result<T, Stop>;

struct Interp<'a> {
    c: &'a Checked,
    out: Vec<u8>,
    input: &'a mut dyn std::io::BufRead,
    steps: u64,
    budget: Option<u64>,
    fired: Vec<bool>,
    /// Per free-integer class: the smallest and largest value ever held.
    stats: HashMap<usize, (i128, i128)>,
    env: Vec<(String, Value)>,
}

pub fn run(checked: &Checked, input: &mut dyn std::io::BufRead, opts: &Options) -> Outcome {
    let mut it = Interp {
        c: checked,
        out: Vec::new(),
        input,
        steps: 0,
        budget: opts.steps,
        fired: vec![false; checked.sites.len()],
        stats: HashMap::new(),
        env: Vec::new(),
    };
    let result = it.exec_block(&checked.program.main);
    let (exit, stderr) = match result {
        Ok(_) => (0, None),
        Err(Stop { code, msg }) => (code, msg),
    };
    let mut trace = Vec::new();
    if opts.trace_checks {
        for (i, s) in checked.sites.iter().enumerate() {
            trace.push(format!(
                "site {} {} {}",
                s.line,
                s.kind.text(),
                if it.fired[i] { "fired" } else { "clean" }
            ));
        }
        for n in &checked.names {
            if let Some(cl) = n.class {
                match it.stats.get(&cl) {
                    Some((lo, hi)) => trace.push(format!("name '{}' min {} max {}", n.name, lo, hi)),
                    None => trace.push(format!("name '{}' unreached", n.name)),
                }
            }
        }
    }
    Outcome { stdout: it.out, stderr, exit, trace }
}

fn fail(code: i32, msg: String) -> Stop {
    Stop { code, msg: Some(msg) }
}

fn free_limit(line: usize) -> Stop {
    fail(5, format!("kespar: oracle limit: a free integer left the i128 range at line {}", line))
}

impl<'a> Interp<'a> {
    fn step(&mut self) -> R<()> {
        self.charge(1)
    }

    /// §8.5 / oracle.md: building text or a list costs one extra step per
    /// 64 bytes, so a loop that builds a string quadratically runs out of
    /// budget rather than memory. `bytes` is a text's UTF-8 length or a
    /// list's element count × 16.
    fn charge_bytes(&mut self, bytes: usize) -> R<()> {
        self.charge((bytes / 64) as u64)
    }

    fn charge_list(&mut self, elems: usize) -> R<()> {
        self.charge_bytes(elems.saturating_mul(16))
    }

    fn charge(&mut self, n: u64) -> R<()> {
        self.steps = self.steps.saturating_add(n);
        if let Some(b) = self.budget {
            if self.steps > b {
                return Err(fail(4, "kespar: step budget exhausted".to_string()));
            }
        }
        Ok(())
    }

    fn info(&self, e: &Expr) -> &'a NodeInfo {
        &self.c.nodes[e.id]
    }

    /// Mark a check site as fired and stop the program.
    fn fire(&mut self, e: &Expr, kind: SiteKind) -> Stop {
        for (k, idx) in &self.info(e).sites {
            if *k == kind {
                self.fired[*idx] = true;
            }
        }
        fail(1, format!("kespar: {} at line {}", kind.text(), e.line))
    }

    fn track(&mut self, e: &Expr, v: &Value) {
        if let (Some(cl), Value::Int(n, _)) = (self.info(e).class, v) {
            let entry = self.stats.entry(cl).or_insert((*n, *n));
            if *n < entry.0 {
                entry.0 = *n;
            }
            if *n > entry.1 {
                entry.1 = *n;
            }
        }
    }

    // ---- environment -----------------------------------------------------

    fn lookup(&self, name: &str) -> Value {
        for (n, v) in self.env.iter().rev() {
            if n == name {
                return v.clone();
            }
        }
        panic!("checker let an undeclared name through: '{}'", name)
    }

    fn assign(&mut self, name: &str, v: Value) {
        for (n, slot) in self.env.iter_mut().rev() {
            if n == name {
                *slot = v;
                return;
            }
        }
        panic!("checker let an undeclared name through: '{}'", name)
    }

    // ---- statements ------------------------------------------------------

    fn exec_block(&mut self, b: &Block) -> R<Flow> {
        let mark = self.env.len();
        let mut flow = Flow::Normal;
        for s in &b.stmts {
            flow = self.exec(s)?;
            if !matches!(flow, Flow::Normal) {
                break;
            }
        }
        self.env.truncate(mark);
        Ok(flow)
    }

    fn exec(&mut self, s: &Stmt) -> R<Flow> {
        self.step()?;
        match &s.kind {
            StmtKind::Decl { name, value, .. } => {
                let v = self.value_or_read(name, value)?;
                self.env.push((name.clone(), v));
            }
            StmtKind::Assign { name, value } => {
                let v = self.value_or_read(name, value)?;
                self.assign(name, v);
            }
            StmtKind::IndexAssign { name, indices, value, .. } => {
                let v = self.eval(value)?;
                let mut target = self.lookup(name);
                for (k, (idx, il, _)) in indices.iter().enumerate() {
                    let i = self.eval_int(idx)?;
                    let list = match &target {
                        Value::List(l) => l.clone(),
                        _ => panic!("checker let a non-list be indexed"),
                    };
                    let len = list.borrow().len() as i128;
                    if i < 0 || i >= len {
                        if let Some(site) = self.c.index_assign_sites.get(&idx.id) {
                            self.fired[*site] = true;
                        }
                        return Err(fail(1, format!("kespar: out of bounds at line {}", il)));
                    }
                    if k + 1 == indices.len() {
                        list.borrow_mut()[i as usize] = v.clone();
                    } else {
                        let next = list.borrow()[i as usize].clone();
                        target = next;
                    }
                }
            }
            StmtKind::If { arms, otherwise } => {
                for (cond, body) in arms {
                    if self.eval_bool(cond)? {
                        return self.exec_block(body);
                    }
                }
                if let Some(b) = otherwise {
                    return self.exec_block(b);
                }
            }
            StmtKind::Loop(body) => loop {
                self.step()?;
                match self.exec_block(body)? {
                    Flow::Break => break,
                    Flow::Return(v) => return Ok(Flow::Return(v)),
                    _ => {}
                }
            },
            StmtKind::While(cond, body) => {
                while self.eval_bool(cond)? {
                    self.step()?;
                    match self.exec_block(body)? {
                        Flow::Break => break,
                        Flow::Return(v) => return Ok(Flow::Return(v)),
                        _ => {}
                    }
                }
            }
            StmtKind::For { var, iter, body } => match &iter.kind {
                ExprKind::Range(a, b) => {
                    self.step()?;
                    let (av, aw) = match self.eval(a)? {
                        Value::Int(v, w) => (v, w),
                        _ => panic!("range bound is not an integer"),
                    };
                    let bv = self.eval_int(b)?;
                    let mut i = av;
                    if av <= bv {
                        loop {
                            self.step()?;
                            let mark = self.env.len();
                            self.env.push((var.clone(), Value::Int(i, aw)));
                            let flow = self.exec_block(body)?;
                            self.env.truncate(mark);
                            match flow {
                                Flow::Break => break,
                                Flow::Return(v) => return Ok(Flow::Return(v)),
                                _ => {}
                            }
                            if i == bv {
                                break;
                            }
                            i += 1;
                        }
                    }
                }
                _ => {
                    let list = match self.eval(iter)? {
                        Value::List(l) => l,
                        _ => panic!("loop.for over a non-list"),
                    };
                    let mut k = 0usize;
                    loop {
                        let elem = {
                            let l = list.borrow();
                            if k >= l.len() {
                                break;
                            }
                            l[k].clone()
                        };
                        self.step()?;
                        let mark = self.env.len();
                        self.env.push((var.clone(), elem));
                        let flow = self.exec_block(body)?;
                        self.env.truncate(mark);
                        match flow {
                            Flow::Break => break,
                            Flow::Return(v) => return Ok(Flow::Return(v)),
                            _ => {}
                        }
                        k += 1;
                    }
                }
            },
            StmtKind::Break => return Ok(Flow::Break),
            StmtKind::Continue => return Ok(Flow::Continue),
            StmtKind::Return(None) => return Ok(Flow::Return(Value::Nothing)),
            StmtKind::Return(Some(e)) => {
                let v = self.eval(e)?;
                return Ok(Flow::Return(v));
            }
            StmtKind::Check(b) | StmtKind::NoCheck(b) => return self.exec_block(b),
            StmtKind::Print { useful, pieces } => {
                let mut text = String::new();
                for p in pieces {
                    match p {
                        Piece::Newline => text.push('\n'),
                        Piece::Expr(e) => {
                            let v = self.eval(e)?;
                            text.push_str(&render_value(&v, *useful));
                        }
                    }
                }
                self.charge_bytes(text.len())?;
                self.out.extend_from_slice(text.as_bytes());
            }
            StmtKind::Exit(e) => {
                let n = self.eval_int(e)?;
                return Err(Stop { code: n as i32, msg: None });
            }
            StmtKind::Call { name, args } => {
                self.call(name, args)?;
            }
        }
        Ok(Flow::Normal)
    }

    fn value_or_read(&mut self, name: &str, value: &Expr) -> R<Value> {
        if let ExprKind::Read(_) = &value.kind {
            self.step()?;
            let info = self.info(value);
            match self.read(&info.rty, &info.read_bounds) {
                Some(v) => {
                    match &v {
                        Value::Str(s) => self.charge_bytes(s.len())?,
                        Value::List(l) => self.charge_list(l.borrow().len())?,
                        _ => {}
                    }
                    Ok(v)
                }
                None => Err(fail(2, format!("kespar: bad input for '{}' at line {}", name, value.line))),
            }
        } else {
            self.eval(value)
        }
    }

    // ---- input (§4) ------------------------------------------------------

    fn next_line(&mut self) -> Option<String> {
        let mut buf = Vec::new();
        match self.input.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => return None,
            Ok(_) => {}
        }
        if buf.last() == Some(&b'\n') {
            buf.pop();
        }
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
        Some(String::from_utf8_lossy(&buf).into_owned())
    }

    fn read(&mut self, rty: &RTy, bounds: &[i128]) -> Option<Value> {
        let line = self.next_line()?;
        match rty {
            RTy::Int(Some(w)) => {
                let v = parse_int_token(line.trim_matches(|c: char| c.is_ascii_whitespace()))?;
                if !w.fits(v) {
                    return None;
                }
                if bounds.len() == 2 && (v < bounds[0] || v > bounds[1]) {
                    return None;
                }
                Some(Value::Int(v, Some(*w)))
            }
            RTy::Bin(BinW::B32) => {
                let t = line.trim_matches(|c: char| c.is_ascii_whitespace());
                t.parse::<f32>().ok().map(Value::Bin32)
            }
            RTy::Bin(BinW::B64) => {
                let t = line.trim_matches(|c: char| c.is_ascii_whitespace());
                t.parse::<f64>().ok().map(Value::Bin64)
            }
            RTy::Bool => match line.trim_matches(|c: char| c.is_ascii_whitespace()) {
                "true" => Some(Value::Bool(true)),
                "false" => Some(Value::Bool(false)),
                _ => None,
            },
            RTy::Str => {
                let count = line.chars().count() as i128;
                if bounds.len() == 2 && (count < bounds[0] || count > bounds[1]) {
                    return None;
                }
                Some(Value::Str(Rc::from(line.as_str())))
            }
            RTy::List(elem) => {
                let w = match &**elem {
                    RTy::Int(Some(w)) => *w,
                    _ => return None,
                };
                let mut items = Vec::new();
                for tok in line.split_ascii_whitespace() {
                    let v = parse_int_token(tok)?;
                    if !w.fits(v) {
                        return None;
                    }
                    if bounds.len() == 4 && (v < bounds[2] || v > bounds[3]) {
                        return None;
                    }
                    items.push(Value::Int(v, Some(w)));
                }
                let len = items.len() as i128;
                if bounds.len() >= 2 && (len < bounds[0] || len > bounds[1]) {
                    return None;
                }
                Some(Value::List(Rc::new(RefCell::new(items))))
            }
            _ => None,
        }
    }

    // ---- expressions -----------------------------------------------------

    fn eval_int(&mut self, e: &Expr) -> R<i128> {
        match self.eval(e)? {
            Value::Int(v, _) => Ok(v),
            _ => panic!("checker let a non-integer through at line {}", e.line),
        }
    }

    fn eval_bool(&mut self, e: &Expr) -> R<bool> {
        match self.eval(e)? {
            Value::Bool(b) => Ok(b),
            _ => panic!("checker let a non-bool condition through at line {}", e.line),
        }
    }

    fn call(&mut self, name: &str, args: &[Expr]) -> R<Value> {
        let fi = self.c.funcs[name];
        let f = &self.c.program.funcs[fi];
        let mut vals = Vec::new();
        for a in args {
            vals.push(self.eval(a)?);
        }
        let saved = std::mem::take(&mut self.env);
        for (p, v) in f.params.iter().zip(vals) {
            self.env.push((p.name.clone(), v));
        }
        let flow = self.exec_block(&f.body);
        self.env = saved;
        match flow? {
            Flow::Return(v) => Ok(v),
            _ => Ok(Value::Nothing),
        }
    }

    fn eval(&mut self, e: &Expr) -> R<Value> {
        self.step()?;
        let info = self.info(e);
        let v = match &e.kind {
            ExprKind::IntLit(v) => match &info.rty {
                RTy::Int(w) => Value::Int(*v, *w),
                RTy::Bin(BinW::B32) => Value::Bin32(*v as f32),
                RTy::Bin(BinW::B64) => Value::Bin64(*v as f64),
                _ => panic!("integer literal with a non-numeric type"),
            },
            ExprKind::BinLit(s) => match &info.rty {
                RTy::Bin(BinW::B32) => Value::Bin32(s.parse().unwrap()),
                _ => Value::Bin64(s.parse().unwrap()),
            },
            ExprKind::TextLit(s) => Value::Str(Rc::from(s.as_str())),
            ExprKind::BoolLit(b) => Value::Bool(*b),
            ExprKind::Name(n) => self.lookup(n),
            ExprKind::Index(base, idx) => {
                let list = match self.eval(base)? {
                    Value::List(l) => l,
                    _ => panic!("indexing a non-list"),
                };
                let i = self.eval_int(idx)?;
                let len = list.borrow().len() as i128;
                if i < 0 || i >= len {
                    return Err(self.fire(e, SiteKind::OutOfBounds));
                }
                let v = list.borrow()[i as usize].clone();
                v
            }
            ExprKind::Call(name, args) => self.call(name, args)?,
            ExprKind::Binary(BinOp::And, l, r) => {
                if !self.eval_bool(l)? {
                    Value::Bool(false)
                } else {
                    Value::Bool(self.eval_bool(r)?)
                }
            }
            ExprKind::Binary(BinOp::Or, l, r) => {
                if self.eval_bool(l)? {
                    Value::Bool(true)
                } else {
                    Value::Bool(self.eval_bool(r)?)
                }
            }
            ExprKind::Binary(op, l, r) => {
                let lv = self.eval(l)?;
                let rv = self.eval(r)?;
                self.binary(e, *op, lv, rv)?
            }
            ExprKind::Unary(UnOp::Neg, x) => match self.eval(x)? {
                Value::Int(v, w) => {
                    let r = v.checked_neg();
                    Value::Int(self.fit(e, r, w)?, w)
                }
                Value::Bin32(v) => Value::Bin32(-v),
                Value::Bin64(v) => Value::Bin64(-v),
                _ => panic!("negating a non-number"),
            },
            ExprKind::Unary(UnOp::Not, x) => Value::Bool(!self.eval_bool(x)?),
            ExprKind::Group(x) => self.eval(x)?,
            ExprKind::ListLit(items) => {
                let mut vals = Vec::new();
                for it in items {
                    vals.push(self.eval(it)?);
                }
                self.charge_list(vals.len())?;
                Value::List(Rc::new(RefCell::new(vals)))
            }
            ExprKind::Pieces(pieces) => {
                let mut text = String::new();
                for p in pieces {
                    match p {
                        Piece::Newline => text.push('\n'),
                        Piece::Expr(pe) => {
                            let v = self.eval(pe)?;
                            text.push_str(&render_value(&v, false));
                        }
                    }
                }
                self.charge_bytes(text.len())?;
                Value::Str(Rc::from(text.as_str()))
            }
            ExprKind::Read(_) | ExprKind::Range(_, _) => panic!("checker let a read or range through as a value"),
            ExprKind::Len(v) => {
                let n = match self.eval(v)? {
                    Value::List(l) => l.borrow().len() as i128,
                    Value::Str(s) => s.chars().count() as i128,
                    _ => panic!("len of a non-list"),
                };
                let w = match &info.rty {
                    RTy::Int(w) => *w,
                    _ => None,
                };
                if let Some(w) = w {
                    if !w.fits(n) {
                        // a length that does not fit the context type: the value cannot be
                        // represented; treat as overflow at this node (§6.5 gives no rule).
                        return Err(self.fire(e, SiteKind::Overflow));
                    }
                }
                Value::Int(n, w)
            }
            ExprKind::Fill(v, n) => {
                let val = self.eval(v)?;
                let count = self.eval_int(n)?;
                if count < 0 {
                    return Err(self.fire(e, SiteKind::OutOfBounds));
                }
                // charged before the list exists, so a huge fill meets the budget, not memory
                let count = usize::try_from(count).unwrap_or(usize::MAX);
                self.charge_list(count)?;
                let mut vals = Vec::with_capacity(count);
                for _ in 0..count {
                    vals.push(val.clone());
                }
                Value::List(Rc::new(RefCell::new(vals)))
            }
            ExprKind::To(spec, v) => {
                let val = self.eval(v)?;
                self.convert(e, spec, val)?
            }
        };
        self.track(e, &v);
        Ok(v)
    }

    /// The exact result must fit the width; a free integer must stay in i128.
    fn fit(&mut self, e: &Expr, r: Option<i128>, w: Option<IntW>) -> R<i128> {
        match (r, w) {
            (Some(v), Some(w)) => {
                if w.fits(v) {
                    Ok(v)
                } else {
                    Err(self.fire(e, SiteKind::Overflow))
                }
            }
            (Some(v), None) => Ok(v),
            (None, Some(_)) => Err(self.fire(e, SiteKind::Overflow)),
            (None, None) => Err(free_limit(e.line)),
        }
    }

    fn binary(&mut self, e: &Expr, op: BinOp, lv: Value, rv: Value) -> R<Value> {
        Ok(match (lv, rv) {
            (Value::Int(a, wa), Value::Int(b, wb)) => {
                let w = wa.or(wb);
                match op {
                    BinOp::Add => Value::Int(self.fit(e, a.checked_add(b), w)?, w),
                    BinOp::Sub => Value::Int(self.fit(e, a.checked_sub(b), w)?, w),
                    BinOp::Mul => Value::Int(self.fit(e, a.checked_mul(b), w)?, w),
                    BinOp::Pow => {
                        if b < 0 {
                            return Err(self.fire(e, SiteKind::NegExp));
                        }
                        let r = pow_i128(a, b);
                        Value::Int(self.fit(e, r, w)?, w)
                    }
                    BinOp::Div => {
                        if b == 0 {
                            return Err(self.fire(e, SiteKind::DivZero));
                        }
                        Value::Int(self.fit(e, a.checked_div(b), w)?, w)
                    }
                    BinOp::Mod => {
                        if b == 0 {
                            return Err(self.fire(e, SiteKind::DivZero));
                        }
                        let r = if b == -1 { 0 } else { a % b };
                        Value::Int(r, w)
                    }
                    BinOp::Eq => Value::Bool(a == b),
                    BinOp::Neq => Value::Bool(a != b),
                    BinOp::Lt => Value::Bool(a < b),
                    BinOp::Gt => Value::Bool(a > b),
                    BinOp::Le => Value::Bool(a <= b),
                    BinOp::Ge => Value::Bool(a >= b),
                    BinOp::And | BinOp::Or => unreachable!(),
                }
            }
            (Value::Bin32(a), Value::Bin32(b)) => match op {
                BinOp::Add => Value::Bin32(a + b),
                BinOp::Sub => Value::Bin32(a - b),
                BinOp::Mul => Value::Bin32(a * b),
                BinOp::Div => Value::Bin32(a / b),
                BinOp::Pow => Value::Bin32(a.powf(b)),
                BinOp::Eq => Value::Bool(a == b),
                BinOp::Neq => Value::Bool(a != b),
                BinOp::Lt => Value::Bool(a < b),
                BinOp::Gt => Value::Bool(a > b),
                BinOp::Le => Value::Bool(a <= b),
                BinOp::Ge => Value::Bool(a >= b),
                _ => panic!("checker let '{}' through on bins", op.spelling()),
            },
            (Value::Bin64(a), Value::Bin64(b)) => match op {
                BinOp::Add => Value::Bin64(a + b),
                BinOp::Sub => Value::Bin64(a - b),
                BinOp::Mul => Value::Bin64(a * b),
                BinOp::Div => Value::Bin64(a / b),
                BinOp::Pow => Value::Bin64(a.powf(b)),
                BinOp::Eq => Value::Bool(a == b),
                BinOp::Neq => Value::Bool(a != b),
                BinOp::Lt => Value::Bool(a < b),
                BinOp::Gt => Value::Bool(a > b),
                BinOp::Le => Value::Bool(a <= b),
                BinOp::Ge => Value::Bool(a >= b),
                _ => panic!("checker let '{}' through on bins", op.spelling()),
            },
            (Value::Bool(a), Value::Bool(b)) => match op {
                BinOp::Eq => Value::Bool(a == b),
                BinOp::Neq => Value::Bool(a != b),
                _ => panic!("checker let '{}' through on bools", op.spelling()),
            },
            (Value::Str(a), Value::Str(b)) => match op {
                BinOp::Eq => Value::Bool(a == b),
                BinOp::Neq => Value::Bool(a != b),
                _ => panic!("checker let '{}' through on strs", op.spelling()),
            },
            (a, b) => panic!(
                "checker let mixed operands through at line {}: {:?} {} {:?}",
                e.line,
                a,
                op.spelling(),
                b
            ),
        })
    }

    /// `std::to` (§6.6).
    fn convert(&mut self, e: &Expr, spec: &TypeSpec, v: Value) -> R<Value> {
        Ok(match (spec, v) {
            (TypeSpec::Int(w), Value::Int(n, _)) => {
                if w.fits(n) {
                    Value::Int(n, Some(*w))
                } else {
                    return Err(self.fire(e, SiteKind::Overflow));
                }
            }
            (TypeSpec::Int(w), Value::Bin32(f)) => self.bin_to_int(e, *w, f as f64)?,
            (TypeSpec::Int(w), Value::Bin64(f)) => self.bin_to_int(e, *w, f)?,
            (TypeSpec::Bin(BinW::B32), Value::Int(n, _)) => Value::Bin32(n as f32),
            (TypeSpec::Bin(BinW::B64), Value::Int(n, _)) => Value::Bin64(n as f64),
            (TypeSpec::Bin(BinW::B32), Value::Bin32(f)) => Value::Bin32(f),
            (TypeSpec::Bin(BinW::B32), Value::Bin64(f)) => Value::Bin32(f as f32),
            (TypeSpec::Bin(BinW::B64), Value::Bin32(f)) => Value::Bin64(f as f64),
            (TypeSpec::Bin(BinW::B64), Value::Bin64(f)) => Value::Bin64(f),
            (TypeSpec::Bool, Value::Bool(b)) => Value::Bool(b),
            (TypeSpec::Str, Value::Str(s)) => Value::Str(s),
            (TypeSpec::Str, v) => {
                let text = render_value(&v, true);
                self.charge_bytes(text.len())?;
                Value::Str(Rc::from(text.as_str()))
            }
            (spec, v) => panic!("checker let std::to.{} through on {:?}", spec.render(), v),
        })
    }

    fn bin_to_int(&mut self, e: &Expr, w: IntW, f: f64) -> R<Value> {
        if !f.is_finite() {
            return Err(self.fire(e, SiteKind::Overflow));
        }
        let t = f.trunc();
        // compare in f64 against the width's range; exact for every width up to 64 bits
        if t < w.min() as f64 || t > w.max() as f64 {
            return Err(self.fire(e, SiteKind::Overflow));
        }
        let n = t as i128;
        if !w.fits(n) {
            return Err(self.fire(e, SiteKind::Overflow));
        }
        Ok(Value::Int(n, Some(w)))
    }
}

/// `a xx b` for `b >= 0`, exactly, or `None` if it leaves i128.
fn pow_i128(a: i128, b: i128) -> Option<i128> {
    if b == 0 {
        return Some(1);
    }
    match a {
        0 => Some(0),
        1 => Some(1),
        -1 => Some(if b % 2 == 0 { 1 } else { -1 }),
        _ => {
            if b > u32::MAX as i128 {
                None
            } else {
                a.checked_pow(b as u32)
            }
        }
    }
}

/// An input integer token: optional `-` and decimal digits, no `_`.
fn parse_int_token(t: &str) -> Option<i128> {
    let digits = t.strip_prefix('-').unwrap_or(t);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    t.parse::<i128>().ok()
}

/// §6.7 rendering.
pub fn render_value(v: &Value, useful: bool) -> String {
    match v {
        Value::Int(n, _) => n.to_string(),
        Value::Bin32(f) => {
            if useful {
                render::bin32_useful(*f)
            } else {
                render::bin32_stored(*f)
            }
        }
        Value::Bin64(f) => {
            if useful {
                render::bin64_useful(*f)
            } else {
                render::bin64_stored(*f)
            }
        }
        Value::Bool(b) => b.to_string(),
        Value::Str(s) => s.to_string(),
        Value::List(l) => {
            let items: Vec<String> = l.borrow().iter().map(|x| render_value(x, useful)).collect();
            format!("[{}]", items.join(", "))
        }
        Value::Nothing => String::new(),
    }
}
