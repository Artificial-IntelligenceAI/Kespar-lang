//! The abstract domain (design/precog.md, layer 2): every value is the set
//! of values it could be. Integers are exact sets up to a cap, then
//! intervals; everything else is "known" or "unknown with what we know".

use std::collections::BTreeSet;

use crate::ast::{BinOp, IntWidth};

/// Sets larger than this become intervals (provisional).
pub const SET_CAP: usize = 4096;

/// "Unbounded" ends of an interval.
pub const NEG_INF: i128 = i128::MIN;
pub const POS_INF: i128 = i128::MAX;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ISet {
    /// Exact, non-empty, at most SET_CAP values.
    Set(BTreeSet<i128>),
    /// lo <= hi; either end may be infinite.
    Range(i128, i128),
    /// No value at all (unreachable).
    Empty,
}

impl ISet {
    pub fn one(v: i128) -> ISet {
        ISet::Set(BTreeSet::from([v]))
    }
    pub fn range(lo: i128, hi: i128) -> ISet {
        if lo > hi {
            return ISet::Empty;
        }
        if lo != NEG_INF && hi != POS_INF && hi.checked_sub(lo).map_or(false, |d| d < SET_CAP as i128) {
            ISet::Set((lo..=hi).collect())
        } else {
            ISet::Range(lo, hi)
        }
    }
    pub fn of_width(w: IntWidth) -> ISet {
        ISet::Range(w.min(), w.max())
    }
    pub fn unbounded() -> ISet {
        ISet::Range(NEG_INF, POS_INF)
    }
    pub fn from_set(s: BTreeSet<i128>) -> ISet {
        if s.is_empty() {
            ISet::Empty
        } else if s.len() > SET_CAP {
            ISet::Range(*s.iter().next().unwrap(), *s.iter().next_back().unwrap())
        } else {
            ISet::Set(s)
        }
    }
    pub fn is_empty(&self) -> bool {
        matches!(self, ISet::Empty)
    }
    pub fn single(&self) -> Option<i128> {
        match self {
            ISet::Set(s) if s.len() == 1 => s.iter().next().copied(),
            _ => None,
        }
    }
    pub fn lo(&self) -> i128 {
        match self {
            ISet::Set(s) => *s.iter().next().unwrap(),
            ISet::Range(lo, _) => *lo,
            ISet::Empty => POS_INF,
        }
    }
    pub fn hi(&self) -> i128 {
        match self {
            ISet::Set(s) => *s.iter().next_back().unwrap(),
            ISet::Range(_, hi) => *hi,
            ISet::Empty => NEG_INF,
        }
    }
    pub fn bounded(&self) -> bool {
        !matches!(self, ISet::Range(lo, hi) if *lo == NEG_INF || *hi == POS_INF)
    }
    pub fn contains(&self, v: i128) -> bool {
        match self {
            ISet::Set(s) => s.contains(&v),
            ISet::Range(lo, hi) => v >= *lo && v <= *hi,
            ISet::Empty => false,
        }
    }
    /// Every value within [lo, hi]?
    pub fn within(&self, lo: i128, hi: i128) -> bool {
        match self {
            ISet::Empty => true,
            _ => self.lo() >= lo && self.hi() <= hi,
        }
    }
    /// No value within [lo, hi]?
    pub fn disjoint(&self, lo: i128, hi: i128) -> bool {
        match self {
            ISet::Empty => true,
            ISet::Set(s) => !s.iter().any(|v| *v >= lo && *v <= hi),
            ISet::Range(a, b) => *b < lo || *a > hi,
        }
    }
    pub fn join(&self, o: &ISet) -> ISet {
        match (self, o) {
            (ISet::Empty, x) | (x, ISet::Empty) => x.clone(),
            (ISet::Set(a), ISet::Set(b)) => ISet::from_set(a.union(b).copied().collect()),
            _ => ISet::Range(self.lo().min(o.lo()), self.hi().max(o.hi())),
        }
    }
    /// Keep only values within [lo, hi].
    pub fn clip(&self, lo: i128, hi: i128) -> ISet {
        match self {
            ISet::Empty => ISet::Empty,
            ISet::Set(s) => ISet::from_set(s.iter().copied().filter(|v| *v >= lo && *v <= hi).collect()),
            ISet::Range(a, b) => ISet::range((*a).max(lo), (*b).min(hi)),
        }
    }
    /// Remove one value (for `!==` narrowing and zero divisors).
    pub fn without(&self, v: i128) -> ISet {
        match self {
            ISet::Set(s) => {
                let mut s = s.clone();
                s.remove(&v);
                ISet::from_set(s)
            }
            ISet::Range(a, b) if *a == v => ISet::range(v + 1, *b),
            ISet::Range(a, b) if *b == v => ISet::range(*a, v - 1),
            other => other.clone(),
        }
    }
    /// Widen: an end that grew goes to the type's end (or infinity).
    pub fn widen(&self, newer: &ISet, floor: i128, ceil: i128) -> ISet {
        if self == newer {
            return self.clone();
        }
        if newer.is_empty() {
            return self.clone();
        }
        if self.is_empty() {
            return newer.clone();
        }
        let lo = if newer.lo() < self.lo() { floor } else { self.lo() };
        let hi = if newer.hi() > self.hi() { ceil } else { self.hi() };
        ISet::Range(lo, hi)
    }
    pub fn iter_values(&self) -> Option<impl Iterator<Item = i128> + '_> {
        match self {
            ISet::Set(s) => Some(s.iter().copied()),
            _ => None,
        }
    }
    pub fn len(&self) -> Option<usize> {
        match self {
            ISet::Set(s) => Some(s.len()),
            ISet::Range(lo, hi) if *lo != NEG_INF && *hi != POS_INF => hi.checked_sub(*lo).and_then(|d| d.checked_add(1)).map(|d| d as usize),
            _ => None,
        }
    }

    fn map2(&self, o: &ISet, f: impl Fn(i128, i128) -> Option<i128>) -> Option<ISet> {
        let (ISet::Set(a), ISet::Set(b)) = (self, o) else { return None };
        if a.len().saturating_mul(b.len()) > SET_CAP * 4 {
            return None;
        }
        let mut out = BTreeSet::new();
        for x in a {
            for y in b {
                out.insert(f(*x, *y)?);
            }
        }
        Some(ISet::from_set(out))
    }

    /// Arithmetic on possibly-unbounded ends; saturates at infinity.
    fn add_inf(a: i128, b: i128) -> i128 {
        if a == NEG_INF || b == NEG_INF { return NEG_INF; }
        if a == POS_INF || b == POS_INF { return POS_INF; }
        a.checked_add(b).unwrap_or(if a > 0 { POS_INF } else { NEG_INF })
    }
    fn mul_inf(a: i128, b: i128) -> i128 {
        if a == 0 || b == 0 { return 0; }
        let neg = (a < 0) != (b < 0);
        if a == NEG_INF || a == POS_INF || b == NEG_INF || b == POS_INF {
            return if neg { NEG_INF } else { POS_INF };
        }
        a.checked_mul(b).unwrap_or(if neg { NEG_INF } else { POS_INF })
    }

    pub fn binop(&self, op: BinOp, o: &ISet) -> ISet {
        if self.is_empty() || o.is_empty() {
            return ISet::Empty;
        }
        match op {
            BinOp::Add => self.map2(o, |a, b| a.checked_add(b)).unwrap_or_else(|| ISet::range(Self::add_inf(self.lo(), o.lo()), Self::add_inf(self.hi(), o.hi()))),
            BinOp::Sub => self.map2(o, |a, b| a.checked_sub(b)).unwrap_or_else(|| ISet::range(Self::add_inf(self.lo(), neg_inf(o.hi())), Self::add_inf(self.hi(), neg_inf(o.lo())))),
            BinOp::Mul => self.map2(o, |a, b| a.checked_mul(b)).unwrap_or_else(|| {
                let c = [Self::mul_inf(self.lo(), o.lo()), Self::mul_inf(self.lo(), o.hi()), Self::mul_inf(self.hi(), o.lo()), Self::mul_inf(self.hi(), o.hi())];
                ISet::range(*c.iter().min().unwrap(), *c.iter().max().unwrap())
            }),
            BinOp::Div => {
                let o = o.without(0);
                if o.is_empty() {
                    return ISet::Empty;
                }
                self.map2(&o, |a, b| a.checked_div(b)).unwrap_or_else(|| {
                    // |result| <= |a|max ; sign depends on both
                    let m = self.lo().checked_abs().unwrap_or(POS_INF).max(self.hi().checked_abs().unwrap_or(POS_INF));
                    if m == POS_INF { ISet::unbounded() } else { ISet::range(-m, m) }
                })
            }
            BinOp::Mod => {
                let o = o.without(0);
                if o.is_empty() {
                    return ISet::Empty;
                }
                self.map2(&o, |a, b| if a == i128::MIN && b == -1 { Some(0) } else { a.checked_rem(b) }).unwrap_or_else(|| {
                    // |a mod b| < |b|, sign of a
                    let bm = o.lo().checked_abs().unwrap_or(POS_INF).max(o.hi().checked_abs().unwrap_or(POS_INF));
                    let bound = if bm == POS_INF { POS_INF } else { bm - 1 };
                    let lo = if self.lo() < 0 { neg_inf(bound) } else { 0 };
                    let hi = if self.hi() > 0 { bound } else { 0 };
                    let am = self.lo().checked_abs().unwrap_or(POS_INF).max(self.hi().checked_abs().unwrap_or(POS_INF));
                    ISet::range(lo.max(neg_inf(am)), hi.min(am))
                })
            }
            BinOp::Pow => {
                let o = o.clip(0, POS_INF);
                if o.is_empty() {
                    return ISet::Empty;
                }
                self.map2(&o, |a, b| pow_checked(a, b)).unwrap_or_else(|| {
                    if self.lo() >= 0 && o.hi() != POS_INF {
                        ISet::range(pow_sat(self.lo(), o.lo()), pow_sat(self.hi(), o.hi()))
                    } else if o.hi() != POS_INF && self.lo() != NEG_INF && self.hi() != POS_INF {
                        let m = pow_sat(self.lo().abs().max(self.hi().abs()), o.hi());
                        ISet::range(neg_inf(m), m)
                    } else {
                        ISet::unbounded()
                    }
                })
            }
            _ => unreachable!(),
        }
    }

    pub fn neg(&self) -> ISet {
        match self {
            ISet::Empty => ISet::Empty,
            ISet::Set(s) => ISet::from_set(s.iter().map(|v| -v).collect()),
            ISet::Range(lo, hi) => ISet::Range(neg_inf(*hi), neg_inf(*lo)),
        }
    }

    /// Compare two sets: (can be true, can be false).
    pub fn cmp(&self, op: BinOp, o: &ISet) -> BSet {
        if self.is_empty() || o.is_empty() {
            return BSet::NONE;
        }
        if let (Some(a), Some(b)) = (self.single(), o.single()) {
            return BSet::of(cmp_val(op, a, b));
        }
        let (alo, ahi, blo, bhi) = (self.lo(), self.hi(), o.lo(), o.hi());
        let (t, f) = match op {
            BinOp::Eq => (!(ahi < blo || alo > bhi) && (self.overlaps(o)), !(alo == ahi && blo == bhi && alo == blo)),
            BinOp::Ne => {
                let eq = self.cmp(BinOp::Eq, o);
                (eq.can_false(), eq.can_true())
            }
            BinOp::Lt => (alo < bhi, ahi >= blo),
            BinOp::Le => (alo <= bhi, ahi > blo),
            BinOp::Gt => (ahi > blo, alo <= bhi),
            BinOp::Ge => (ahi >= blo, alo < bhi),
            _ => unreachable!(),
        };
        BSet::new(t, f)
    }

    fn overlaps(&self, o: &ISet) -> bool {
        match (self, o) {
            (ISet::Set(a), ISet::Set(b)) => a.iter().any(|v| b.contains(v)),
            _ => !(self.hi() < o.lo() || self.lo() > o.hi()),
        }
    }

    /// Narrow `self` assuming `self op o` is `truth`.
    pub fn narrow(&self, op: BinOp, o: &ISet, truth: bool) -> ISet {
        if o.is_empty() {
            return ISet::Empty;
        }
        let op = if truth { op } else { match op {
            BinOp::Eq => BinOp::Ne, BinOp::Ne => BinOp::Eq, BinOp::Lt => BinOp::Ge, BinOp::Ge => BinOp::Lt,
            BinOp::Gt => BinOp::Le, BinOp::Le => BinOp::Gt, _ => unreachable!(),
        } };
        match op {
            BinOp::Eq => match (self, o) {
                (ISet::Set(a), ISet::Set(b)) => ISet::from_set(a.intersection(b).copied().collect()),
                _ => self.clip(o.lo(), o.hi()),
            },
            BinOp::Ne => match o.single() {
                Some(v) => self.without(v),
                None => self.clone(),
            },
            BinOp::Lt => self.clip(NEG_INF, sub1(o.hi())),
            BinOp::Le => self.clip(NEG_INF, o.hi()),
            BinOp::Gt => self.clip(add1(o.lo()), POS_INF),
            BinOp::Ge => self.clip(o.lo(), POS_INF),
            _ => unreachable!(),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            ISet::Empty => "nothing".into(),
            ISet::Set(s) if s.len() == 1 => format!("{}", s.iter().next().unwrap()),
            ISet::Set(s) if s.len() <= 6 => format!("{{{}}}", s.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ")),
            _ => format!("{}..{}", fmt_inf(self.lo()), fmt_inf(self.hi())),
        }
    }
}

fn fmt_inf(v: i128) -> String {
    if v == NEG_INF { "-inf".into() } else if v == POS_INF { "inf".into() } else { v.to_string() }
}

fn neg_inf(v: i128) -> i128 {
    if v == NEG_INF { POS_INF } else if v == POS_INF { NEG_INF } else { -v }
}
fn add1(v: i128) -> i128 {
    if v == POS_INF || v == NEG_INF { v } else { v + 1 }
}
fn sub1(v: i128) -> i128 {
    if v == POS_INF || v == NEG_INF { v } else { v - 1 }
}

pub fn cmp_val(op: BinOp, a: i128, b: i128) -> bool {
    match op {
        BinOp::Eq => a == b, BinOp::Ne => a != b, BinOp::Lt => a < b, BinOp::Le => a <= b,
        BinOp::Gt => a > b, BinOp::Ge => a >= b, _ => unreachable!(),
    }
}

fn pow_checked(a: i128, b: i128) -> Option<i128> {
    if b < 0 { return None; }
    if b == 0 { return Some(1); }
    match a { 0 => return Some(0), 1 => return Some(1), -1 => return Some(if b % 2 == 0 { 1 } else { -1 }), _ => {} }
    if b > 127 { return None; }
    let mut r: i128 = 1;
    for _ in 0..b {
        r = r.checked_mul(a)?;
    }
    Some(r)
}

fn pow_sat(a: i128, b: i128) -> i128 {
    pow_checked(a, b).unwrap_or(if a < 0 && b % 2 == 1 { NEG_INF } else { POS_INF })
}

/// A set of booleans, as two bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BSet(u8);

impl BSet {
    pub const NONE: BSet = BSet(0);
    pub const TRUE: BSet = BSet(1);
    pub const FALSE: BSet = BSet(2);
    pub const BOTH: BSet = BSet(3);
    pub fn of(b: bool) -> BSet {
        if b { BSet::TRUE } else { BSet::FALSE }
    }
    pub fn new(t: bool, f: bool) -> BSet {
        BSet((t as u8) | ((f as u8) << 1))
    }
    pub fn can_true(self) -> bool {
        self.0 & 1 != 0
    }
    pub fn can_false(self) -> bool {
        self.0 & 2 != 0
    }
    pub fn single(self) -> Option<bool> {
        match self.0 { 1 => Some(true), 2 => Some(false), _ => None }
    }
    pub fn join(self, o: BSet) -> BSet {
        BSet(self.0 | o.0)
    }
    pub fn not(self) -> BSet {
        BSet::new(self.can_false(), self.can_true())
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// What is known about a bin.
#[derive(Debug, Clone, PartialEq)]
pub enum ABin {
    Known(f64),
    Unknown,
}

impl ABin {
    pub fn join(&self, o: &ABin) -> ABin {
        match (self, o) {
            (ABin::Known(a), ABin::Known(b)) if a.to_bits() == b.to_bits() => ABin::Known(*a),
            _ => ABin::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AStr {
    pub known: Option<String>,
    pub len: ISet,
}

impl AStr {
    pub fn known(s: String) -> AStr {
        let n = s.chars().count() as i128;
        AStr { known: Some(s), len: ISet::one(n) }
    }
    pub fn join(&self, o: &AStr) -> AStr {
        match (&self.known, &o.known) {
            (Some(a), Some(b)) if a == b => self.clone(),
            _ => AStr { known: None, len: self.len.join(&o.len) },
        }
    }
}

/// A list value: which abstract objects it may be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AListRef(pub BTreeSet<u32>);

#[derive(Debug, Clone, PartialEq)]
pub enum AVal {
    Int(ISet),
    Bin(ABin),
    Bool(BSet),
    Str(AStr),
    List(AListRef),
    Nothing,
    /// Not yet assigned on some path (a declaration inside a branch).
    Undef,
}

impl AVal {
    pub fn join(&self, o: &AVal) -> AVal {
        match (self, o) {
            (AVal::Undef, x) | (x, AVal::Undef) => x.clone(),
            (AVal::Int(a), AVal::Int(b)) => AVal::Int(a.join(b)),
            (AVal::Bin(a), AVal::Bin(b)) => AVal::Bin(a.join(b)),
            (AVal::Bool(a), AVal::Bool(b)) => AVal::Bool(a.join(*b)),
            (AVal::Str(a), AVal::Str(b)) => AVal::Str(a.join(b)),
            (AVal::List(a), AVal::List(b)) => AVal::List(AListRef(a.0.union(&b.0).copied().collect())),
            (AVal::Nothing, AVal::Nothing) => AVal::Nothing,
            (a, b) => unreachable!("join of {a:?} and {b:?}"),
        }
    }
    pub fn is_empty(&self) -> bool {
        match self {
            AVal::Int(s) => s.is_empty(),
            AVal::Bool(b) => b.is_empty(),
            AVal::List(r) => r.0.is_empty(),
            _ => false,
        }
    }
    pub fn as_int(&self) -> &ISet {
        match self {
            AVal::Int(s) => s,
            other => unreachable!("not an int: {other:?}"),
        }
    }
    pub fn as_bool(&self) -> BSet {
        match self {
            AVal::Bool(b) => *b,
            other => unreachable!("not a bool: {other:?}"),
        }
    }
}

/// An abstract list object.
#[derive(Debug, Clone, PartialEq)]
pub struct AList {
    pub len: ISet,
    /// Every element joined.
    pub elem: AVal,
    /// Per-element values while the length is known and small (strong updates).
    pub exact: Option<Vec<AVal>>,
    /// Allocated once on this path so far: strong updates are sound.
    pub single: bool,
}

impl AList {
    pub fn join(&self, o: &AList) -> AList {
        let exact = match (&self.exact, &o.exact) {
            (Some(a), Some(b)) if a.len() == b.len() => Some(a.iter().zip(b).map(|(x, y)| x.join(y)).collect()),
            _ => None,
        };
        AList { len: self.len.join(&o.len), elem: self.elem.join(&o.elem), exact, single: self.single && o.single }
    }
}
