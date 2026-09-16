//! Types with inference variables, and the union-find table that decides
//! `:=` names (language.md §8.3 step 1 — forcing). Step 2 (widths of free
//! names) is Precog's, in `precog/`.

use crate::ast::{BinWidth, IntWidth};
use crate::diag::{CompileError, Result};

/// A resolved type. `Int(Free(id))` is a `:=` integer whose width Precog
/// picks; the reference interpreter treats it as unbounded.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Ty {
    Int(IntTy),
    Bin(BinWidth),
    Bool,
    Str,
    List(Box<Ty>),
    Nothing,
    /// An inference variable; only appears before `Types::norm`.
    Var(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IntTy {
    Fixed(IntWidth),
    Free(u32),
}

impl Ty {
    pub fn name(&self) -> String {
        match self {
            Ty::Int(IntTy::Fixed(w)) => w.name().to_string(),
            Ty::Int(IntTy::Free(_)) => "integer (width chosen by Precog)".into(),
            Ty::Bin(w) => w.name().to_string(),
            Ty::Bool => "bool".into(),
            Ty::Str => "str.utf8".into(),
            Ty::List(t) => format!("list.{}", t.name()),
            Ty::Nothing => "nothing".into(),
            Ty::Var(_) => "?".into(),
        }
    }
    pub fn is_int(&self) -> bool {
        matches!(self, Ty::Int(_))
    }
    pub fn is_bin(&self) -> bool {
        matches!(self, Ty::Bin(_))
    }
    pub fn int_width(&self) -> Option<IntWidth> {
        match self {
            Ty::Int(IntTy::Fixed(w)) => Some(*w),
            _ => None,
        }
    }
}

/// What a variable is allowed to become.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Any,
    /// An unadorned number literal: an integer, or a bin if the context says so.
    NumLit,
    IntLike,
    BinLike,
}

#[derive(Debug, Clone)]
struct Entry {
    parent: u32,
    binding: Option<Ty>,
    kind: Kind,
    /// What this variable stands for, for messages: `'total'`, `the literal 5`.
    what: String,
    line: u32,
    /// Where it was bound to a concrete type, for the "used as X and as Y" message.
    bound_line: u32,
    /// Assigned in `finish` to unbound integer roots.
    free_id: Option<u32>,
}

#[derive(Debug, Default, Clone)]
pub struct Types {
    entries: Vec<Entry>,
    /// Origins of free integers, indexed by free id: (what, line).
    pub frees: Vec<(String, u32)>,
}

impl Types {
    pub fn fresh(&mut self, kind: Kind, what: impl Into<String>, line: u32) -> Ty {
        let id = self.entries.len() as u32;
        self.entries.push(Entry { parent: id, binding: None, kind, what: what.into(), line, bound_line: 0, free_id: None });
        Ty::Var(id)
    }

    fn find(&mut self, mut v: u32) -> u32 {
        while self.entries[v as usize].parent != v {
            let p = self.entries[v as usize].parent;
            self.entries[v as usize].parent = self.entries[p as usize].parent;
            v = p;
        }
        v
    }

    /// Follow bindings until a concrete constructor or an unbound root.
    pub fn resolve(&mut self, t: &Ty) -> Ty {
        match t {
            Ty::Var(v) => {
                let r = self.find(*v);
                match self.entries[r as usize].binding.clone() {
                    Some(b) => self.resolve(&b),
                    None => Ty::Var(r),
                }
            }
            Ty::List(e) => Ty::List(Box::new(self.resolve(e))),
            other => other.clone(),
        }
    }

    pub fn kind_of(&mut self, v: u32) -> Kind {
        let r = self.find(v);
        self.entries[r as usize].kind
    }

    /// Make two types the same, or say why they cannot be.
    pub fn unify(&mut self, a: &Ty, b: &Ty, line: u32) -> Result<()> {
        let a = self.resolve(a);
        let b = self.resolve(b);
        match (&a, &b) {
            (Ty::Var(x), Ty::Var(y)) if x == y => Ok(()),
            (Ty::Var(x), Ty::Var(y)) => {
                let (x, y) = (*x, *y);
                let kx = self.entries[x as usize].kind;
                let ky = self.entries[y as usize].kind;
                let merged = match (kx, ky) {
                    (Kind::Any, k) | (k, Kind::Any) => k,
                    (Kind::NumLit, k) | (k, Kind::NumLit) => k,
                    (Kind::IntLike, Kind::IntLike) => Kind::IntLike,
                    (Kind::BinLike, Kind::BinLike) => Kind::BinLike,
                    _ => {
                        let (wx, wy) = (self.entries[x as usize].what.clone(), self.entries[y as usize].what.clone());
                        return Err(CompileError::new(line, format!("{wx} is an integer but {wy} is a bin; nothing converts implicitly (use `std::to`)")));
                    }
                };
                // A named origin (`'total'`) beats a literal's, so messages and
                // the width report name the declaration; otherwise the older one.
                let named = |e: &Entry| e.what.starts_with('\'');
                let (nx, ny) = (named(&self.entries[x as usize]), named(&self.entries[y as usize]));
                let (root, child) = if nx != ny { if nx { (x, y) } else { (y, x) } } else if x < y { (x, y) } else { (y, x) };
                self.entries[child as usize].parent = root;
                self.entries[root as usize].kind = merged;
                Ok(())
            }
            (Ty::Var(x), t) | (t, Ty::Var(x)) => {
                let x = *x;
                let kind = self.entries[x as usize].kind;
                let ok = match (kind, t) {
                    (Kind::Any, _) => true,
                    (Kind::NumLit, Ty::Int(_)) | (Kind::NumLit, Ty::Bin(_)) => true,
                    (Kind::IntLike, Ty::Int(_)) => true,
                    (Kind::BinLike, Ty::Bin(_)) => true,
                    _ => false,
                };
                if !ok {
                    let what = self.entries[x as usize].what.clone();
                    let family = match kind { Kind::IntLike => "an integer", Kind::BinLike => "a bin", _ => "a number" };
                    return Err(CompileError::new(line, format!("{what} is {family} but is used as {}; nothing converts implicitly (use `std::to`)", t.name())));
                }
                if let Ty::Nothing = t {
                    let what = self.entries[x as usize].what.clone();
                    return Err(CompileError::new(line, format!("{what} would be given a call that returns nothing")));
                }
                self.entries[x as usize].binding = Some(t.clone());
                self.entries[x as usize].bound_line = line;
                Ok(())
            }
            (Ty::List(x), Ty::List(y)) => self.unify(x, y, line),
            _ if a == b => Ok(()),
            _ => {
                // Which name was forced? Look for the variable behind `a` or `b`.
                Err(CompileError::new(line, format!("{} meets {}; nothing converts implicitly (use `std::to`)", a.name(), b.name())))
            }
        }
    }

    /// Like `unify` but words the failure as §8.3's forcing conflict when a
    /// `:=` name is involved: "'x' is used as int32 at line 4 and as int64 at line 9".
    pub fn force(&mut self, what: &Ty, with: &Ty, line: u32) -> Result<()> {
        let before = self.resolve(what);
        match self.unify(what, with, line) {
            Ok(()) => Ok(()),
            Err(e) => {
                if let Ty::Var(_) = before {
                    return Err(e);
                }
                if let Some((name, bl)) = self.origin_of(what) {
                    if bl != 0 {
                        let w = self.resolve(with);
                        return Err(CompileError::new(line, format!("{name} is used as {} at line {bl} and as {} at line {line}", before.name(), w.name())));
                    }
                }
                Err(e)
            }
        }
    }

    /// An untyped function with no `return [v]`: its return variable becomes `nothing`.
    pub fn bind_nothing(&mut self, v: u32) {
        let r = self.find(v);
        self.entries[r as usize].binding = Some(Ty::Nothing);
    }

    fn origin_of(&mut self, t: &Ty) -> Option<(String, u32)> {
        if let Ty::Var(v) = t {
            let r = self.find(*v);
            let e = &self.entries[r as usize];
            return Some((e.what.clone(), e.bound_line));
        }
        None
    }

    /// After checking: every unbound root becomes a free integer (or bin64),
    /// and `norm` maps `Var` away for good.
    pub fn finish(&mut self) -> Result<()> {
        for i in 0..self.entries.len() as u32 {
            let r = self.find(i);
            if r != i || self.entries[r as usize].binding.is_some() {
                continue;
            }
            let e = &self.entries[r as usize];
            match e.kind {
                Kind::IntLike | Kind::NumLit => {
                    let id = self.frees.len() as u32;
                    self.frees.push((e.what.clone(), e.line));
                    self.entries[r as usize].free_id = Some(id);
                }
                Kind::BinLike => {
                    self.entries[r as usize].binding = Some(Ty::Bin(BinWidth::B64));
                }
                Kind::Any => {
                    return Err(CompileError::new(e.line, format!("cannot tell the type of {} from any use; write one", e.what)));
                }
            }
        }
        Ok(())
    }

    /// The final type: no `Var` left.
    pub fn norm(&mut self, t: &Ty) -> Ty {
        match self.resolve(t) {
            Ty::Var(r) => match self.entries[r as usize].free_id {
                Some(id) => Ty::Int(IntTy::Free(id)),
                None => panic!("norm before finish"),
            },
            Ty::List(e) => Ty::List(Box::new(self.norm(&e))),
            t => t,
        }
    }
}
