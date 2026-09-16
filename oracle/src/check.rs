//! The static checker: types, no implicit conversion, `[ ]` rules,
//! shadowing, immut, arity, MAIN, the boundary rule, and §8.3 step-1
//! forcing of `:=` names. Produces the per-node information the
//! interpreter runs on, and the check-site list (§8.4) in source order.

use std::collections::HashMap;

use crate::ast::*;
use crate::CompileError;

// ---- families: what an unbound type variable may still become ---------------

const INT: u8 = 1;
const BIN: u8 = 2;
const BOOL: u8 = 4;
const STR: u8 = 8;
const LIST: u8 = 16;
const NUM: u8 = INT | BIN;
const ALL: u8 = INT | BIN | BOOL | STR | LIST;

fn fam_name(f: u8) -> String {
    let mut parts = Vec::new();
    if f & INT != 0 {
        parts.push("integer");
    }
    if f & BIN != 0 {
        parts.push("bin");
    }
    if f & BOOL != 0 {
        parts.push("bool");
    }
    if f & STR != 0 {
        parts.push("str");
    }
    if f & LIST != 0 {
        parts.push("list");
    }
    if parts.is_empty() {
        "nothing".to_string()
    } else {
        parts.join(" or ")
    }
}

/// A checker type. `Var` is a type variable: a `:=` name's leaf, a literal, or
/// `std::len`'s result.
#[derive(Clone, Debug, PartialEq)]
pub enum Ty {
    Int(IntW),
    Bin(BinW),
    Bool,
    Str,
    List(Box<Ty>),
    Var(usize),
    /// A call to a function that returns nothing.
    Nothing,
}

#[derive(Clone, Debug)]
struct Var {
    parent: usize,
    fam: u8,
    bound: Option<(Ty, usize)>,
    /// The `:=` name this variable belongs to, if any.
    name: Option<String>,
}

/// The run-time type of a node, after forcing.
#[derive(Clone, Debug, PartialEq)]
pub enum RTy {
    /// `None` width: a free integer — unbounded (§8.3 step 2).
    Int(Option<IntW>),
    Bin(BinW),
    Bool,
    Str,
    List(Box<RTy>),
    Nothing,
}

impl RTy {
    pub fn from_spec(s: &TypeSpec) -> RTy {
        match s {
            TypeSpec::Int(w) => RTy::Int(Some(*w)),
            TypeSpec::Bin(w) => RTy::Bin(*w),
            TypeSpec::Bool => RTy::Bool,
            TypeSpec::Str => RTy::Str,
            TypeSpec::List(t) => RTy::List(Box::new(RTy::from_spec(t))),
        }
    }
    pub fn render(&self) -> String {
        match self {
            RTy::Int(Some(w)) => w.name().to_string(),
            RTy::Int(None) => "free".to_string(),
            RTy::Bin(w) => w.name().to_string(),
            RTy::Bool => "bool".to_string(),
            RTy::Str => "str.utf8".to_string(),
            RTy::List(t) => format!("list.{}", t.render()),
            RTy::Nothing => "nothing".to_string(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SiteKind {
    Overflow,
    DivZero,
    NegExp,
    OutOfBounds,
}

impl SiteKind {
    pub fn text(self) -> &'static str {
        match self {
            SiteKind::Overflow => "overflow",
            SiteKind::DivZero => "division by zero",
            SiteKind::NegExp => "negative exponent",
            SiteKind::OutOfBounds => "out of bounds",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Site {
    pub line: usize,
    pub col: usize,
    pub kind: SiteKind,
}

/// Per-expression information for the interpreter.
#[derive(Clone, Debug)]
pub struct NodeInfo {
    pub rty: RTy,
    /// For a free integer node: the class of the free name(s) whose width it shares.
    pub class: Option<usize>,
    /// Check sites at this node: (kind, site index).
    pub sites: Vec<(SiteKind, usize)>,
    /// For a `std::read.stdin` node: the bounds as written.
    pub read_bounds: Vec<i128>,
}

impl Default for NodeInfo {
    fn default() -> NodeInfo {
        NodeInfo { rty: RTy::Nothing, class: None, sites: Vec::new(), read_bounds: Vec::new() }
    }
}

/// A `:=` name and its forcing result.
#[derive(Clone, Debug)]
pub struct NamedVar {
    pub name: String,
    pub line: usize,
    pub col: usize,
    pub rty: RTy,
    /// For a free integer name: its class (shared by every name forced to it).
    pub class: Option<usize>,
    /// False when some leaf of the name's type was forced by nothing (§8.3 step 2).
    pub forced: bool,
}

pub struct Checked {
    pub program: Program,
    pub funcs: HashMap<String, usize>,
    pub nodes: Vec<NodeInfo>,
    pub sites: Vec<Site>,
    pub names: Vec<NamedVar>,
    /// The out-of-bounds site of each index in an element assignment, by the index expression id.
    pub index_assign_sites: HashMap<usize, usize>,
}

impl Checked {
    pub fn types_listing(&self) -> Vec<String> {
        self.names
            .iter()
            .map(|n| {
                let t = if n.forced { n.rty.render() } else { "free".to_string() };
                format!("'{}' {}", n.name, t)
            })
            .collect()
    }
}

#[derive(Clone, Debug)]
struct Binding {
    name: String,
    ty: Ty,
    spec: Option<TypeSpec>,
    immut: bool,
    is_for: bool,
}

#[derive(Clone, Debug)]
struct FuncSig {
    params: Vec<(Ty, bool, String, Option<TypeSpec>)>,
    /// `None`: the function returns nothing.
    ret: Option<Ty>,
}

struct Checker {
    vars: Vec<Var>,
    node_ty: Vec<Ty>,
    scopes: Vec<Vec<Binding>>,
    sigs: HashMap<String, FuncSig>,
    cur_func: Option<String>,
    loop_depth: usize,
    /// `:=` names in declaration order: (name, line, col, type).
    named: Vec<(String, usize, usize, Ty)>,
    /// `std::to` nodes to check against the conversion table once types are known.
    to_checks: Vec<(usize, TypeSpec, Ty, usize)>,
    /// Untyped-declaration `:=` nodes whose literal family narrows to integer.
    /// (line, col, order, node id, kind, is_index_assign)
    sites: Vec<(usize, usize, u8, usize, SiteKind, bool)>,
    nodes: Vec<NodeInfo>,
}

pub fn check(program: Program) -> Result<Checked, CompileError> {
    let mut c = Checker {
        vars: Vec::new(),
        node_ty: vec![Ty::Nothing; program.expr_count],
        scopes: Vec::new(),
        sigs: HashMap::new(),
        cur_func: None,
        loop_depth: 0,
        named: Vec::new(),
        to_checks: Vec::new(),
        sites: Vec::new(),
        nodes: vec![NodeInfo::default(); program.expr_count],
    };
    c.signatures(&program)?;
    for f in &program.funcs {
        c.func_body(f)?;
    }
    c.cur_func = None;
    c.scopes.clear();
    c.block(&program.main)?;
    c.resolve_all(&program)?;
    let mut funcs = HashMap::new();
    for (i, f) in program.funcs.iter().enumerate() {
        funcs.insert(f.name.clone(), i);
    }
    // order sites by source position (line, col, kind order)
    let mut order: Vec<usize> = (0..c.sites.len()).collect();
    order.sort_by_key(|&i| (c.sites[i].0, c.sites[i].1, c.sites[i].2));
    let mut sites = Vec::new();
    let mut index_assign_sites = HashMap::new();
    for (idx, &i) in order.iter().enumerate() {
        let (line, col, _, node, kind, ia) = c.sites[i];
        sites.push(Site { line, col, kind });
        if ia {
            index_assign_sites.insert(node, idx);
        } else {
            c.nodes[node].sites.push((kind, idx));
        }
    }
    let mut names = Vec::new();
    for (name, line, col, ty) in c.named.clone() {
        let rty = c.resolve_ty(&ty);
        let class = c.class_of(&ty);
        let forced = !c.has_free_leaf(&ty);
        names.push(NamedVar { name, line, col, rty, class, forced });
    }
    names.sort_by_key(|n| (n.line, n.col));
    Ok(Checked { program, funcs, nodes: c.nodes, sites, names, index_assign_sites })
}

fn spec_to_ty(s: &TypeSpec) -> Ty {
    match s {
        TypeSpec::Int(w) => Ty::Int(*w),
        TypeSpec::Bin(w) => Ty::Bin(*w),
        TypeSpec::Bool => Ty::Bool,
        TypeSpec::Str => Ty::Str,
        TypeSpec::List(t) => Ty::List(Box::new(spec_to_ty(t))),
    }
}

fn err<T>(msg: &str, line: usize) -> Result<T, CompileError> {
    Err(CompileError::new(msg, line))
}

fn is_bare_number(e: &Expr) -> bool {
    matches!(e.kind, ExprKind::IntLit(_) | ExprKind::BinLit(_))
}

/// Does a `return [v]` appear anywhere in the block?
fn has_value_return(b: &Block) -> bool {
    b.stmts.iter().any(|s| match &s.kind {
        StmtKind::Return(Some(_)) => true,
        StmtKind::If { arms, otherwise } => {
            arms.iter().any(|(_, b)| has_value_return(b))
                || otherwise.as_ref().map_or(false, has_value_return)
        }
        StmtKind::Loop(b) | StmtKind::While(_, b) | StmtKind::Check(b) | StmtKind::NoCheck(b) => {
            has_value_return(b)
        }
        StmtKind::For { body, .. } => has_value_return(body),
        _ => false,
    })
}

/// Does every path through the block return (or exit)?
fn block_returns(b: &Block) -> bool {
    b.stmts.iter().any(stmt_returns)
}

fn stmt_returns(s: &Stmt) -> bool {
    match &s.kind {
        StmtKind::Return(_) | StmtKind::Exit(_) => true,
        StmtKind::If { arms, otherwise } => {
            otherwise.as_ref().map_or(false, block_returns) && arms.iter().all(|(_, b)| block_returns(b))
        }
        StmtKind::Loop(b) => !has_break(b),
        StmtKind::Check(b) | StmtKind::NoCheck(b) => block_returns(b),
        _ => false,
    }
}

/// A `break` that targets this block's loop (not one nested inside another loop).
fn has_break(b: &Block) -> bool {
    b.stmts.iter().any(|s| match &s.kind {
        StmtKind::Break => true,
        StmtKind::If { arms, otherwise } => {
            arms.iter().any(|(_, b)| has_break(b)) || otherwise.as_ref().map_or(false, has_break)
        }
        StmtKind::Check(b) | StmtKind::NoCheck(b) => has_break(b),
        _ => false,
    })
}

impl Checker {
    // ---- type variables ----------------------------------------------------

    fn new_var(&mut self, fam: u8, name: Option<String>) -> usize {
        let id = self.vars.len();
        self.vars.push(Var { parent: id, fam, bound: None, name });
        id
    }

    fn find(&mut self, v: usize) -> usize {
        let mut r = v;
        while self.vars[r].parent != r {
            r = self.vars[r].parent;
        }
        let mut x = v;
        while self.vars[x].parent != x {
            let p = self.vars[x].parent;
            self.vars[x].parent = r;
            x = p;
        }
        r
    }

    /// Follow a bound variable to its type (one level: the result is never a bound Var).
    fn shallow(&mut self, t: &Ty) -> Ty {
        let mut t = t.clone();
        loop {
            match t {
                Ty::Var(v) => {
                    let r = self.find(v);
                    match self.vars[r].bound.clone() {
                        Some((b, _)) => t = b,
                        None => return Ty::Var(r),
                    }
                }
                _ => return t,
            }
        }
    }

    fn fam_of(&mut self, t: &Ty) -> u8 {
        match self.shallow(t) {
            Ty::Int(_) => INT,
            Ty::Bin(_) => BIN,
            Ty::Bool => BOOL,
            Ty::Str => STR,
            Ty::List(_) => LIST,
            Ty::Var(r) => self.vars[r].fam,
            Ty::Nothing => 0,
        }
    }

    fn render(&mut self, t: &Ty) -> String {
        match self.shallow(t) {
            Ty::Int(w) => w.name().to_string(),
            Ty::Bin(w) => w.name().to_string(),
            Ty::Bool => "bool".to_string(),
            Ty::Str => "str.utf8".to_string(),
            Ty::List(e) => format!("list.{}", self.render(&e)),
            Ty::Var(r) => {
                let f = self.vars[r].fam;
                match &self.vars[r].name {
                    Some(n) => format!("'{}' ({})", n, fam_name(f)),
                    None => format!("untyped {}", fam_name(f)),
                }
            }
            Ty::Nothing => "nothing".to_string(),
        }
    }

    fn mismatch<T>(&mut self, a: &Ty, b: &Ty, line: usize) -> Result<T, CompileError> {
        let ra = self.render(a);
        let rb = self.render(b);
        err(&format!("cannot mix {} and {}: there is no implicit conversion (std::to converts)", ra, rb), line)
    }

    /// Bind an unbound root variable to a non-variable type.
    fn bind(&mut self, r: usize, t: &Ty, line: usize) -> Result<(), CompileError> {
        if let Ty::Nothing = t {
            return err("a function that returns nothing cannot be used as a value", line);
        }
        let need = self.fam_of(t);
        let have = self.vars[r].fam;
        if have & need == 0 {
            return self.mismatch(&Ty::Var(r), t, line);
        }
        self.vars[r].fam = have & need;
        self.vars[r].bound = Some((t.clone(), line));
        Ok(())
    }

    /// Two values meet (§8.3 step 1): they must have exactly one type.
    fn unify(&mut self, a: &Ty, b: &Ty, line: usize) -> Result<Ty, CompileError> {
        match (a, b) {
            (Ty::Var(x), Ty::Var(y)) => {
                let rx = self.find(*x);
                let ry = self.find(*y);
                if rx == ry {
                    return Ok(Ty::Var(rx));
                }
                let bx = self.vars[rx].bound.clone();
                let by = self.vars[ry].bound.clone();
                match (bx, by) {
                    (Some((tx, lx)), Some((ty, _))) => {
                        if let Err(e) = self.unify(&tx, &ty, line) {
                            return Err(self.forcing_error(*x, rx, &tx, lx, &ty, line, e));
                        }
                    }
                    (Some((tx, _)), None) => self.bind(ry, &tx, line)?,
                    (None, Some((ty, _))) => self.bind(rx, &ty, line)?,
                    (None, None) => {
                        let f = self.vars[rx].fam & self.vars[ry].fam;
                        if f == 0 {
                            return self.mismatch(&Ty::Var(rx), &Ty::Var(ry), line);
                        }
                        self.vars[rx].fam = f;
                        self.vars[ry].fam = f;
                    }
                }
                // union: a named variable stays the representative
                let (root, other) = if self.vars[rx].name.is_some() || self.vars[ry].name.is_none() {
                    (rx, ry)
                } else {
                    (ry, rx)
                };
                let f = self.vars[rx].fam & self.vars[ry].fam;
                if self.vars[root].bound.is_none() {
                    self.vars[root].bound = self.vars[other].bound.clone();
                }
                self.vars[root].fam = f;
                self.vars[other].parent = root;
                Ok(Ty::Var(root))
            }
            (Ty::Var(x), t) | (t, Ty::Var(x)) => {
                let rx = self.find(*x);
                match self.vars[rx].bound.clone() {
                    Some((bt, bl)) => {
                        if let Err(e) = self.unify(&bt, t, line) {
                            return Err(self.forcing_error(*x, rx, &bt, bl, t, line, e));
                        }
                    }
                    None => self.bind(rx, t, line)?,
                }
                Ok(Ty::Var(rx))
            }
            (Ty::Int(x), Ty::Int(y)) if x == y => Ok(a.clone()),
            (Ty::Bin(x), Ty::Bin(y)) if x == y => Ok(a.clone()),
            (Ty::Bool, Ty::Bool) | (Ty::Str, Ty::Str) => Ok(a.clone()),
            (Ty::List(x), Ty::List(y)) => Ok(Ty::List(Box::new(self.unify(x, y, line)?))),
            (Ty::Nothing, _) | (_, Ty::Nothing) => {
                err("a function that returns nothing cannot be used as a value", line)
            }
            _ => self.mismatch(a, b, line),
        }
    }

    /// A named `:=` variable already forced to one type meets another.
    fn forcing_error(&mut self, x: usize, r: usize, was: &Ty, was_line: usize, now: &Ty, line: usize, inner: CompileError) -> CompileError {
        let sw = self.shallow(was);
        let sn = self.shallow(now);
        let same_family = matches!((&sw, &sn), (Ty::Int(_), Ty::Int(_)) | (Ty::Bin(_), Ty::Bin(_)));
        let name = self.vars[x].name.clone().or_else(|| self.vars[r].name.clone());
        match (&name, same_family) {
            (Some(n), true) => {
                let n = n.clone();
                let a = self.render(was);
                let b = self.render(now);
                CompileError::new(&format!("'{}' is used as {} at line {} and as {}", n, a, was_line, b), line)
            }
            _ => inner,
        }
    }

    /// Restrict a type to a family; an unbound variable narrows.
    fn require_fam(&mut self, t: &Ty, mask: u8, what: &str, line: usize) -> Result<(), CompileError> {
        let s = self.shallow(t);
        if let Ty::Var(r) = s {
            let f = self.vars[r].fam & mask;
            if f == 0 {
                let rt = self.render(t);
                return err(&format!("{} cannot take {}", what, rt), line);
            }
            self.vars[r].fam = f;
            return Ok(());
        }
        if let Ty::Nothing = s {
            return err("a function that returns nothing cannot be used as a value", line);
        }
        let f = self.fam_of(&s);
        if f & mask == 0 {
            let rt = self.render(t);
            return err(&format!("{} cannot take {}", what, rt), line);
        }
        Ok(())
    }

    /// The element type of a list type (binding an unbound variable to a list).
    fn elem_of(&mut self, t: &Ty, line: usize) -> Result<Ty, CompileError> {
        match self.shallow(t) {
            Ty::List(e) => Ok(*e),
            Ty::Var(r) => {
                if self.vars[r].fam & LIST == 0 {
                    let rt = self.render(t);
                    return err(&format!("{} is not a list", rt), line);
                }
                let e = self.new_var(ALL, None);
                self.bind(r, &Ty::List(Box::new(Ty::Var(e))), line)?;
                Ok(Ty::Var(e))
            }
            other => {
                let rt = self.render(&other);
                err(&format!("{} is not a list", rt), line)
            }
        }
    }

    /// Give a `:=` name its type: the initial expression's shape with a named
    /// variable at every leaf (§8.3: family fixed by the initial expression).
    fn name_type(&mut self, t: &Ty, name: &str, line: usize) -> Result<Ty, CompileError> {
        match self.shallow(t) {
            Ty::Int(w) => {
                let v = self.new_var(INT, Some(name.to_string()));
                self.vars[v].bound = Some((Ty::Int(w), line));
                Ok(Ty::Var(v))
            }
            Ty::Bin(w) => {
                let v = self.new_var(BIN, Some(name.to_string()));
                self.vars[v].bound = Some((Ty::Bin(w), line));
                Ok(Ty::Var(v))
            }
            Ty::Bool => Ok(Ty::Bool),
            Ty::Str => Ok(Ty::Str),
            Ty::List(e) => Ok(Ty::List(Box::new(self.name_type(&e, name, line)?))),
            Ty::Var(r) => {
                let mut f = self.vars[r].fam;
                if f == NUM {
                    // an integer-looking literal fixes the integer family
                    f = INT;
                }
                let v = self.new_var(f, Some(name.to_string()));
                self.unify(&Ty::Var(v), &Ty::Var(r), line)
            }
            Ty::Nothing => err("a function that returns nothing cannot be used as a value", line),
        }
    }

    // ---- scopes ----------------------------------------------------------------

    fn lookup(&self, name: &str) -> Option<&Binding> {
        for scope in self.scopes.iter().rev() {
            for b in scope.iter().rev() {
                if b.name == name {
                    return Some(b);
                }
            }
        }
        None
    }

    fn declare(&mut self, b: Binding) {
        self.scopes.last_mut().unwrap().push(b);
    }

    /// The shadowing rule (§3).
    fn shadow_check(&self, name: &str, shadow: bool, line: usize) -> Result<(), CompileError> {
        match self.lookup(name) {
            Some(v) => {
                if v.immut {
                    return err(&format!("'{}' is immut and cannot be shadowed", name), line);
                }
                if !shadow {
                    return err(
                        &format!("'{}' is already declared: shadowing is never silent, write $:= or $= to hide it", name),
                        line,
                    );
                }
                Ok(())
            }
            None => {
                if shadow {
                    return err(&format!("$ with nothing to hide: '{}' is not visible", name), line);
                }
                Ok(())
            }
        }
    }

    // ---- functions -------------------------------------------------------------

    fn signatures(&mut self, p: &Program) -> Result<(), CompileError> {
        for f in &p.funcs {
            if self.sigs.contains_key(&f.name) {
                return err(&format!("function '{}' is defined twice", f.name), f.line);
            }
            let mut params = Vec::new();
            for prm in &f.params {
                if params.iter().any(|(_, _, n, _): &(Ty, bool, String, Option<TypeSpec>)| n == &prm.name) {
                    return err(&format!("parameter '{}' appears twice", prm.name), prm.line);
                }
                let ty = match &prm.ty {
                    Some(spec) => spec_to_ty(spec),
                    None => {
                        let v = self.new_var(ALL, Some(prm.name.clone()));
                        self.named.push((prm.name.clone(), prm.line, prm.col, Ty::Var(v)));
                        Ty::Var(v)
                    }
                };
                params.push((ty, prm.immut, prm.name.clone(), prm.ty.clone()));
            }
            let ret = match &f.ret {
                Some(spec) => Some(spec_to_ty(spec)),
                None => {
                    if has_value_return(&f.body) {
                        let n = format!("{}()", f.name);
                        let v = self.new_var(ALL, Some(n.clone()));
                        self.named.push((n, f.line, f.col, Ty::Var(v)));
                        Some(Ty::Var(v))
                    } else {
                        None
                    }
                }
            };
            self.sigs.insert(f.name.clone(), FuncSig { params, ret });
        }
        Ok(())
    }

    fn func_body(&mut self, f: &Func) -> Result<(), CompileError> {
        let sig = self.sigs[&f.name].clone();
        self.cur_func = Some(f.name.clone());
        self.loop_depth = 0;
        self.scopes.clear();
        self.scopes.push(Vec::new());
        for (ty, immut, name, spec) in &sig.params {
            self.declare(Binding { name: name.clone(), ty: ty.clone(), spec: spec.clone(), immut: *immut, is_for: false });
        }
        self.block(&f.body)?;
        self.scopes.pop();
        if sig.ret.is_some() && !block_returns(&f.body) {
            return err(&format!("not every path of '{}' returns a value", f.name), f.line);
        }
        Ok(())
    }

    // ---- statements ------------------------------------------------------------

    fn block(&mut self, b: &Block) -> Result<(), CompileError> {
        self.scopes.push(Vec::new());
        for s in &b.stmts {
            self.stmt(s)?;
        }
        self.scopes.pop();
        Ok(())
    }

    fn stmt(&mut self, s: &Stmt) -> Result<(), CompileError> {
        let line = s.line;
        match &s.kind {
            StmtKind::Decl { name, ty, immut, shadow, value } => {
                self.shadow_check(name, *shadow, line)?;
                match ty {
                    Some(spec) => {
                        let t = spec_to_ty(spec);
                        if let ExprKind::Read(bounds) = &value.kind {
                            self.check_read(spec, bounds, value)?;
                            self.node_ty[value.id] = t.clone();
                        } else {
                            let vt = self.expr(value)?;
                            self.unify(&t, &vt, line)?;
                        }
                        self.declare(Binding { name: name.clone(), ty: t, spec: Some(spec.clone()), immut: *immut, is_for: false });
                    }
                    None => {
                        if let ExprKind::Read(_) = &value.kind {
                            return err(&format!("'{}' comes from outside the program; write its type", name), line);
                        }
                        let vt = self.expr(value)?;
                        let nt = self.name_type(&vt, name, line)?;
                        self.named.push((name.clone(), value.line, value.col, nt.clone()));
                        self.declare(Binding { name: name.clone(), ty: nt, spec: None, immut: *immut, is_for: false });
                    }
                }
            }
            StmtKind::Assign { name, value } => {
                let b = match self.lookup(name) {
                    Some(b) => b.clone(),
                    None => return err(&format!("'{}' is not declared", name), line),
                };
                if b.immut {
                    return err(&format!("'{}' is immut and cannot be assigned", name), line);
                }
                if b.is_for {
                    return err(&format!("'{}' is a loop.for variable and cannot be assigned", name), line);
                }
                if let ExprKind::Read(bounds) = &value.kind {
                    match &b.spec {
                        Some(spec) => {
                            self.check_read(spec, bounds, value)?;
                            self.node_ty[value.id] = b.ty.clone();
                        }
                        None => return err(&format!("'{}' comes from outside the program; write its type", name), line),
                    }
                } else {
                    let vt = self.expr(value)?;
                    self.unify(&b.ty, &vt, line)?;
                }
            }
            StmtKind::IndexAssign { name, indices, value, .. } => {
                let b = match self.lookup(name) {
                    Some(b) => b.clone(),
                    None => return err(&format!("'{}' is not declared", name), line),
                };
                let mut t = b.ty.clone();
                for (idx, il, _) in indices {
                    let it = self.expr(idx)?;
                    self.require_fam(&it, INT, "an index", *il)?;
                    t = self.elem_of(&t, *il)?;
                }
                let vt = self.expr(value)?;
                self.unify(&t, &vt, line)?;
            }
            StmtKind::If { arms, otherwise } => {
                for (cond, body) in arms {
                    self.condition(cond)?;
                    self.block(body)?;
                }
                if let Some(b) = otherwise {
                    self.block(b)?;
                }
            }
            StmtKind::Loop(b) => {
                self.loop_depth += 1;
                self.block(b)?;
                self.loop_depth -= 1;
            }
            StmtKind::While(cond, b) => {
                self.condition(cond)?;
                self.loop_depth += 1;
                self.block(b)?;
                self.loop_depth -= 1;
            }
            StmtKind::For { var, iter, body } => {
                let elem = match &iter.kind {
                    ExprKind::Range(a, b) => {
                        let at = self.expr(a)?;
                        let bt = self.expr(b)?;
                        let t = self.unify(&at, &bt, iter.line)?;
                        self.require_fam(&t, INT, "std::range", iter.line)?;
                        self.node_ty[iter.id] = t.clone();
                        t
                    }
                    _ => {
                        let it = self.expr(iter)?;
                        self.elem_of(&it, iter.line)?
                    }
                };
                self.shadow_check(var, false, line)?;
                let vt = self.name_type(&elem, var, line)?;
                self.named.push((var.clone(), line, 0, vt.clone()));
                self.scopes.push(Vec::new());
                self.declare(Binding { name: var.clone(), ty: vt, spec: None, immut: false, is_for: true });
                self.loop_depth += 1;
                self.block(body)?;
                self.loop_depth -= 1;
                self.scopes.pop();
            }
            StmtKind::Break => {
                if self.loop_depth == 0 {
                    return err("break outside a loop", line);
                }
            }
            StmtKind::Continue => {
                if self.loop_depth == 0 {
                    return err("continue outside a loop", line);
                }
            }
            StmtKind::Return(v) => {
                let fname = match &self.cur_func {
                    Some(f) => f.clone(),
                    None => return err("return outside a function (MAIN is not a function)", line),
                };
                let ret = self.sigs[&fname].ret.clone();
                match (ret, v) {
                    (None, Some(_)) => return err(&format!("'{}' returns nothing", fname), line),
                    (Some(_), None) => return err(&format!("'{}' returns a value: write return [v]", fname), line),
                    (Some(rt), Some(e)) => {
                        let vt = self.expr(e)?;
                        self.unify(&rt, &vt, line)?;
                    }
                    (None, None) => {}
                }
            }
            StmtKind::Check(b) | StmtKind::NoCheck(b) => self.block(b)?,
            StmtKind::Print { pieces, .. } => {
                for p in pieces {
                    if let Piece::Expr(e) = p {
                        if is_bare_number(e) {
                            return err("a bare number is an operand, never a piece: write a name or a text", e.line);
                        }
                        let t = self.expr(e)?;
                        self.require_fam(&t, ALL, "a piece", e.line)?;
                    }
                }
            }
            StmtKind::Exit(e) => {
                let t = self.expr(e)?;
                self.unify(&Ty::Int(IntW::U8), &t, line)?;
            }
            StmtKind::Call { name, args } => {
                self.call(name, args, line)?;
            }
        }
        Ok(())
    }

    fn condition(&mut self, cond: &Expr) -> Result<(), CompileError> {
        let t = self.expr(cond)?;
        let s = self.shallow(&t);
        if s != Ty::Bool {
            let r = self.render(&t);
            return err(&format!("a condition must be bool, not {}", r), cond.line);
        }
        Ok(())
    }

    fn call(&mut self, name: &str, args: &[Expr], line: usize) -> Result<Ty, CompileError> {
        let sig = match self.sigs.get(name) {
            Some(s) => s.clone(),
            None => return err(&format!("function '{}' is not declared", name), line),
        };
        if sig.params.len() != args.len() {
            return err(
                &format!("'{}' takes {} argument(s), {} given", name, sig.params.len(), args.len()),
                line,
            );
        }
        for ((pt, _, _, _), a) in sig.params.iter().zip(args) {
            let at = self.expr(a)?;
            self.unify(pt, &at, a.line)?;
        }
        Ok(sig.ret.clone().unwrap_or(Ty::Nothing))
    }

    /// §4: the bounds of a read, against the declared type.
    fn check_read(&mut self, spec: &TypeSpec, bounds: &[Expr], node: &Expr) -> Result<(), CompileError> {
        let line = node.line;
        let mut vals = Vec::new();
        for b in bounds {
            let v = match &b.kind {
                ExprKind::IntLit(v) => *v,
                ExprKind::Unary(UnOp::Neg, inner) => match &inner.kind {
                    ExprKind::IntLit(v) => -*v,
                    _ => return err("a read bound must be an integer literal", line),
                },
                _ => return err("a read bound must be an integer literal", line),
            };
            vals.push(v);
        }
        let fit = |v: i128, w: IntW, line: usize| -> Result<(), CompileError> {
            if w.fits(v) {
                Ok(())
            } else {
                err(&format!("literal {} does not fit {}", v, w.name()), line)
            }
        };
        match spec {
            TypeSpec::Int(w) => {
                if !(vals.is_empty() || vals.len() == 2) {
                    return err("an integer read takes no bound or [lo, hi]", line);
                }
                for &v in &vals {
                    fit(v, *w, line)?;
                }
            }
            TypeSpec::Bin(_) | TypeSpec::Bool => {
                if !vals.is_empty() {
                    return err("bins and bools take no read bound", line);
                }
            }
            TypeSpec::Str => {
                if !(vals.is_empty() || vals.len() == 2) {
                    return err("a str read takes no bound or [lo, hi]", line);
                }
                for &v in &vals {
                    if v < 0 {
                        return err("a str read bound must be non-negative", line);
                    }
                }
            }
            TypeSpec::List(e) => {
                let ew = match &**e {
                    TypeSpec::Int(w) => *w,
                    other => {
                        return err(&format!("list.{} cannot be read; only a list of an integer type can", other.render()), line)
                    }
                };
                if !(vals.is_empty() || vals.len() == 2 || vals.len() == 4) {
                    return err("a list read takes no bound, [lo, hi] or [lo, hi, elo, ehi]", line);
                }
                if vals.len() >= 2 && (vals[0] < 0 || vals[1] < 0) {
                    return err("a list length bound must be non-negative", line);
                }
                if vals.len() == 4 {
                    fit(vals[2], ew, line)?;
                    fit(vals[3], ew, line)?;
                    if vals[2] > vals[3] {
                        return err("a read bound needs lo <= hi", line);
                    }
                }
            }
        }
        if vals.len() >= 2 && vals[0] > vals[1] {
            return err("a read bound needs lo <= hi", line);
        }
        self.nodes[node.id].read_bounds = vals;
        Ok(())
    }

    // ---- expressions -----------------------------------------------------------

    fn expr(&mut self, e: &Expr) -> Result<Ty, CompileError> {
        let line = e.line;
        let ty = match &e.kind {
            ExprKind::IntLit(_) => Ty::Var(self.new_var(NUM, None)),
            ExprKind::BinLit(_) => Ty::Var(self.new_var(BIN, None)),
            ExprKind::TextLit(_) => Ty::Str,
            ExprKind::BoolLit(_) => Ty::Bool,
            ExprKind::Name(n) => match self.lookup(n) {
                Some(b) => b.ty.clone(),
                None => return err(&format!("'{}' is not declared", n), line),
            },
            ExprKind::Index(base, idx) => {
                let bt = self.expr(base)?;
                let it = self.expr(idx)?;
                self.require_fam(&it, INT, "an index", line)?;
                self.elem_of(&bt, line)?
            }
            ExprKind::Call(name, args) => {
                let t = self.call(name, args, line)?;
                if t == Ty::Nothing {
                    return err(&format!("'{}' returns nothing and cannot be used as a value", name), line);
                }
                t
            }
            ExprKind::Binary(op, l, r) => {
                let lt = self.expr(l)?;
                let rt = self.expr(r)?;
                match op {
                    BinOp::And | BinOp::Or => {
                        self.unify(&Ty::Bool, &lt, line).map_err(|_| {
                            CompileError::new(&format!("'{}' takes two bools", op.spelling()), line)
                        })?;
                        self.unify(&Ty::Bool, &rt, line).map_err(|_| {
                            CompileError::new(&format!("'{}' takes two bools", op.spelling()), line)
                        })?;
                        Ty::Bool
                    }
                    BinOp::Mod => {
                        let t = self.unify(&lt, &rt, line)?;
                        self.require_fam(&t, INT, "'mod' (integers only; mod on bins is a compile error)", line)?;
                        t
                    }
                    BinOp::Eq | BinOp::Neq => {
                        let t = self.unify(&lt, &rt, line)?;
                        self.require_fam(&t, INT | BIN | BOOL | STR, &format!("'{}' (lists cannot be compared)", op.spelling()), line)?;
                        Ty::Bool
                    }
                    _ if op.is_ordered_cmp() => {
                        let t = self.unify(&lt, &rt, line)?;
                        self.require_fam(&t, NUM, &format!("'{}'", op.spelling()), line)?;
                        Ty::Bool
                    }
                    _ => {
                        let t = self.unify(&lt, &rt, line)?;
                        self.require_fam(&t, NUM, &format!("'{}'", op.spelling()), line)?;
                        t
                    }
                }
            }
            ExprKind::Unary(UnOp::Neg, x) => {
                let t = self.expr(x)?;
                self.require_fam(&t, NUM, "unary '-'", line)?;
                t
            }
            ExprKind::Unary(UnOp::Not, x) => {
                let t = self.expr(x)?;
                self.unify(&Ty::Bool, &t, line)
                    .map_err(|_| CompileError::new("'not' takes a bool", line))?;
                Ty::Bool
            }
            ExprKind::Group(x) => self.expr(x)?,
            ExprKind::ListLit(items) => {
                let mut t = self.expr(&items[0])?;
                self.require_fam(&t, ALL, "a list element", items[0].line)?;
                for it in &items[1..] {
                    let et = self.expr(it)?;
                    t = self.unify(&t, &et, it.line).map_err(|e| {
                        CompileError::new(&format!("a list literal's elements must all be one type: {}", e.msg), it.line)
                    })?;
                }
                Ty::List(Box::new(t))
            }
            ExprKind::Pieces(pieces) => {
                for p in pieces {
                    if let Piece::Expr(pe) = p {
                        if is_bare_number(pe) {
                            return err("a bare number is an operand, never a piece: write a name or a text", pe.line);
                        }
                        let t = self.expr(pe)?;
                        self.require_fam(&t, ALL, "a piece", pe.line)?;
                    }
                }
                Ty::Str
            }
            ExprKind::Read(_) => {
                return err(
                    "std::read.stdin may appear only as the whole value of a typed declaration or typed assignment",
                    line,
                )
            }
            ExprKind::Range(_, _) => return err("std::range is a value only loop.for accepts", line),
            ExprKind::Len(v) => {
                let t = self.expr(v)?;
                self.require_fam(&t, STR | LIST, "std::len", line)?;
                Ty::Var(self.new_var(INT, None))
            }
            ExprKind::Fill(v, n) => {
                let vt = self.expr(v)?;
                self.require_fam(&vt, ALL, "std::fill's value", line)?;
                let nt = self.expr(n)?;
                self.require_fam(&nt, INT, "std::fill's count", line)?;
                Ty::List(Box::new(vt))
            }
            ExprKind::To(spec, v) => {
                let vt = self.expr(v)?;
                self.require_fam(&vt, ALL, "std::to", line)?;
                self.to_checks.push((e.id, spec.clone(), vt, line));
                spec_to_ty(spec)
            }
        };
        self.node_ty[e.id] = ty.clone();
        Ok(ty)
    }

    // ---- pass 2: resolution ----------------------------------------------------

    fn resolve_ty(&mut self, t: &Ty) -> RTy {
        match self.shallow(t) {
            Ty::Int(w) => RTy::Int(Some(w)),
            Ty::Bin(w) => RTy::Bin(w),
            Ty::Bool => RTy::Bool,
            Ty::Str => RTy::Str,
            Ty::List(e) => RTy::List(Box::new(self.resolve_ty(&e))),
            Ty::Var(r) => {
                let f = self.vars[r].fam;
                if f & INT != 0 {
                    RTy::Int(None)
                } else if f & BIN != 0 {
                    RTy::Bin(BinW::B64)
                } else {
                    RTy::Int(None)
                }
            }
            Ty::Nothing => RTy::Nothing,
        }
    }

    /// Is any leaf of the type an unbound variable?
    fn has_free_leaf(&mut self, t: &Ty) -> bool {
        match self.shallow(t) {
            Ty::Var(_) => true,
            Ty::List(e) => self.has_free_leaf(&e),
            _ => false,
        }
    }

    /// The free-integer class of a type, if it is (or is a list of) a free integer.
    fn class_of(&mut self, t: &Ty) -> Option<usize> {
        match self.shallow(t) {
            Ty::Var(r) => {
                let f = self.vars[r].fam;
                if f & INT != 0 {
                    Some(r)
                } else {
                    None
                }
            }
            Ty::List(e) => self.class_of(&e),
            _ => None,
        }
    }

    fn resolve_all(&mut self, p: &Program) -> Result<(), CompileError> {
        for (id, spec, vt, line) in self.to_checks.clone() {
            let from = self.resolve_ty(&vt);
            let ok = match (&from, &spec) {
                (RTy::Str, TypeSpec::Str) => true,
                (RTy::Str, _) => false,
                (RTy::List(_), _) | (_, TypeSpec::List(_)) => false,
                (RTy::Bool, TypeSpec::Bool) => true,
                (RTy::Bool, TypeSpec::Str) => true,
                (RTy::Bool, _) => false,
                (_, TypeSpec::Bool) => false,
                (RTy::Nothing, _) => false,
                _ => true,
            };
            if !ok {
                return err(&format!("std::to cannot convert {} to {}", from.render(), spec.render()), line);
            }
            let _ = id;
        }
        for f in &p.funcs {
            self.resolve_block(&f.body)?;
        }
        self.resolve_block(&p.main)?;
        Ok(())
    }

    fn resolve_block(&mut self, b: &Block) -> Result<(), CompileError> {
        for s in &b.stmts {
            self.resolve_stmt(s)?;
        }
        Ok(())
    }

    fn resolve_stmt(&mut self, s: &Stmt) -> Result<(), CompileError> {
        match &s.kind {
            StmtKind::Decl { value, .. } | StmtKind::Assign { value, .. } => self.resolve_expr(value)?,
            StmtKind::IndexAssign { indices, value, .. } => {
                for (idx, il, ic) in indices {
                    self.resolve_expr(idx)?;
                    self.sites.push((*il, *ic, 0, idx.id, SiteKind::OutOfBounds, true));
                }
                self.resolve_expr(value)?;
            }
            StmtKind::If { arms, otherwise } => {
                for (c, b) in arms {
                    self.resolve_expr(c)?;
                    self.resolve_block(b)?;
                }
                if let Some(b) = otherwise {
                    self.resolve_block(b)?;
                }
            }
            StmtKind::Loop(b) | StmtKind::Check(b) | StmtKind::NoCheck(b) => self.resolve_block(b)?,
            StmtKind::While(c, b) => {
                self.resolve_expr(c)?;
                self.resolve_block(b)?;
            }
            StmtKind::For { iter, body, .. } => {
                self.resolve_expr(iter)?;
                self.resolve_block(body)?;
            }
            StmtKind::Return(Some(e)) | StmtKind::Exit(e) => self.resolve_expr(e)?,
            StmtKind::Print { pieces, .. } => {
                for p in pieces {
                    if let Piece::Expr(e) = p {
                        self.resolve_expr(e)?;
                    }
                }
            }
            StmtKind::Call { args, .. } => {
                for a in args {
                    self.resolve_expr(a)?;
                }
            }
            StmtKind::Return(None) | StmtKind::Break | StmtKind::Continue => {}
        }
        Ok(())
    }

    fn resolve_expr(&mut self, e: &Expr) -> Result<(), CompileError> {
        let ty = self.node_ty[e.id].clone();
        let rty = self.resolve_ty(&ty);
        let class = if rty == RTy::Int(None) { self.class_of(&ty) } else { None };
        let line = e.line;
        let col = e.col;
        let mut sites = Vec::new();
        match &e.kind {
            ExprKind::IntLit(v) => match &rty {
                RTy::Int(Some(w)) => {
                    if !w.fits(*v) {
                        return err(&format!("literal {} does not fit {}", v, w.name()), line);
                    }
                }
                RTy::Bin(BinW::B32) => {
                    if !(*v as f32).is_finite() {
                        return err(&format!("literal {} does not fit bin32", v), line);
                    }
                }
                _ => {}
            },
            ExprKind::BinLit(s) => {
                let fits = match &rty {
                    RTy::Bin(BinW::B32) => s.parse::<f32>().map(|v| v.is_finite()).unwrap_or(false),
                    _ => s.parse::<f64>().map(|v| v.is_finite()).unwrap_or(false),
                };
                if !fits {
                    return err(&format!("literal {} does not fit {}", s, rty.render()), line);
                }
            }
            ExprKind::Index(base, idx) => {
                self.resolve_expr(base)?;
                self.resolve_expr(idx)?;
                sites.push(SiteKind::OutOfBounds);
            }
            ExprKind::Call(_, args) => {
                for a in args {
                    self.resolve_expr(a)?;
                }
            }
            ExprKind::Binary(op, l, r) => {
                self.resolve_expr(l)?;
                self.resolve_expr(r)?;
                if op.is_arith() {
                    if let RTy::Int(_) = rty {
                        match op {
                            BinOp::Pow => {
                                sites.push(SiteKind::NegExp);
                                sites.push(SiteKind::Overflow);
                            }
                            BinOp::Div | BinOp::Mod => {
                                sites.push(SiteKind::DivZero);
                                sites.push(SiteKind::Overflow);
                            }
                            _ => sites.push(SiteKind::Overflow),
                        }
                    }
                }
            }
            ExprKind::Unary(UnOp::Neg, x) => {
                self.resolve_expr(x)?;
                if let RTy::Int(_) = rty {
                    sites.push(SiteKind::Overflow);
                }
            }
            ExprKind::Unary(UnOp::Not, x) | ExprKind::Group(x) => self.resolve_expr(x)?,
            ExprKind::Len(x) => {
                self.resolve_expr(x)?;
                if let RTy::Int(Some(_)) = rty {
                    sites.push(SiteKind::Overflow);
                }
            }
            ExprKind::ListLit(items) => {
                for it in items {
                    self.resolve_expr(it)?;
                }
            }
            ExprKind::Pieces(pieces) => {
                for p in pieces {
                    if let Piece::Expr(pe) = p {
                        self.resolve_expr(pe)?;
                    }
                }
            }
            ExprKind::Range(a, b) => {
                self.resolve_expr(a)?;
                self.resolve_expr(b)?;
            }
            ExprKind::Fill(v, n) => {
                self.resolve_expr(v)?;
                self.resolve_expr(n)?;
                sites.push(SiteKind::OutOfBounds);
            }
            ExprKind::To(spec, v) => {
                self.resolve_expr(v)?;
                if let TypeSpec::Int(w) = spec {
                    let from = self.nodes[v.id].rty.clone();
                    let same = from == RTy::Int(Some(*w));
                    if !same {
                        sites.push(SiteKind::Overflow);
                    }
                }
            }
            ExprKind::Read(_) | ExprKind::TextLit(_) | ExprKind::BoolLit(_) | ExprKind::Name(_) => {}
        }
        for (k, kind) in sites.into_iter().enumerate() {
            self.sites.push((line, col, k as u8, e.id, kind, false));
        }
        self.nodes[e.id].rty = rty;
        self.nodes[e.id].class = class;
        Ok(())
    }
}
