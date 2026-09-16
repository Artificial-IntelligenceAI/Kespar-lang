//! Precog: what the compiler knows about the program before it runs.
//! design/precog.md. Layer 1 runs a read-free program to the end; layer 2
//! runs every program over sets; layer 3 (`smt`) asks Z3 about what is left.

pub mod domain;
pub mod interp;

use std::collections::HashMap;

use crate::ast::{IntWidth, Tier};
use crate::diag::{CompileError, Result};
use crate::emit::{Decisions, Known};
use crate::ir::{Program, SiteId};
use crate::types::{IntTy, Ty};
use crate::vm::Outcome;
use domain::{ABin, AVal, ISet};

/// Compile-time execution budget in steps (provisional, language.md §8.5).
pub const STEP_BUDGET: u64 = 100_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// No check: the property holds on every path (or the site is never reached).
    Proven,
    /// A run-time check stays (default tier).
    Checked,
    /// Removed unverified (nocheck).
    Trusted,
    /// The whole program ran at compile time and this site was passed.
    Ran,
}

#[derive(Debug, Clone)]
pub struct SiteReport {
    pub id: SiteId,
    pub line: u32,
    pub func: String,
    pub what: String,
    pub kind: &'static str,
    pub verdict: Verdict,
    pub note: String,
}

#[derive(Debug, Clone)]
pub struct WidthReport {
    pub name: String,
    pub line: u32,
    pub width: IntWidth,
    pub note: String,
}

pub struct Report {
    pub decisions: Decisions,
    pub sites: Vec<SiteReport>,
    pub widths: Vec<WidthReport>,
    /// Layer 1 ran the whole program.
    pub whole: bool,
    pub steps: u64,
}

pub fn analyse(prog: &Program) -> Result<Report> {
    if !prog.has_reads() {
        return whole_run(prog);
    }
    bounded_run(prog)
}

fn narrowest(lo: i128, hi: i128) -> Option<IntWidth> {
    IntWidth::ALL.iter().copied().find(|w| w.holds(lo) && w.holds(hi))
}

fn width_for(prog: &Program, id: usize, lo: i128, hi: i128, at: u32) -> Result<IntWidth> {
    let (name, line) = &prog.free_names[id];
    match narrowest(lo, hi) {
        Some(w) => Ok(w),
        None => {
            let big = if hi.abs() > lo.abs() { hi } else { lo };
            Err(CompileError::new(*line, format!("no integer type holds {name} (reaches {big} at line {at}); write an explicit type")))
        }
    }
}

/// Layer 1: no reads — run it, and the program becomes its output.
fn whole_run(prog: &Program) -> Result<Report> {
    let dec = Decisions {
        site_checked: vec![true; prog.sites.len()],
        widths: vec![None; prog.free_names.len()],
        track_frees: true,
        known: HashMap::new(),
        whole: None,
    };
    let module = crate::emit::emit(prog, &dec);
    let r = crate::vm::run(&module, Box::new(std::io::empty()), Some(STEP_BUDGET));
    match &r.outcome {
        Outcome::Exit(_) => {}
        Outcome::Trap { kind, line, site } => {
            if let Some(s) = site {
                if let Some(fid) = prog.sites[*s as usize].free {
                    let (name, _) = &prog.free_names[fid as usize];
                    return Err(CompileError::new(*line, format!("no integer type holds {name} (past 2^127 at line {line}); write an explicit type")));
                }
            }
            return Err(CompileError::new(*line, format!("this program always stops with {} (it was run at compile time)", kind.text())));
        }
        Outcome::BadInput { .. } => unreachable!("no reads"),
        Outcome::Budget => {
            return Err(CompileError::new(prog.funcs[prog.main as usize].line, format!("compile-time execution exceeded {STEP_BUDGET} steps; the program may not finish")));
        }
    }
    let mut widths = Vec::new();
    let mut wdec = Vec::new();
    for (i, rng) in r.free_ranges.iter().enumerate() {
        let (lo, hi) = rng.unwrap_or((0, 0));
        let w = width_for(prog, i, lo, hi, prog.free_names[i].1)?;
        let (name, line) = &prog.free_names[i];
        widths.push(WidthReport { name: name.clone(), line: *line, width: w, note: if rng.is_some() { format!("held {lo}..{hi}") } else { "never held a value".into() } });
        wdec.push(Some(w));
    }
    let sites = prog.sites.iter().filter(|s| s.active).map(|s| SiteReport {
        id: s.id, line: s.line, func: prog.funcs[s.func as usize].name.clone(), what: s.what.clone(), kind: s.kind.text(),
        verdict: if s.tier == Tier::Nocheck { Verdict::Trusted } else { Verdict::Ran },
        note: "the whole program ran at compile time".into(),
    }).collect();
    let decisions = Decisions {
        site_checked: vec![false; prog.sites.len()],
        widths: wdec,
        track_frees: false,
        known: HashMap::new(),
        whole: Some((r.stdout, r.outcome)),
    };
    Ok(Report { decisions, sites, widths, whole: true, steps: r.steps })
}

/// Layer 2 (and 3): reads exist — run over sets, prove what can be proven.
fn bounded_run(prog: &Program) -> Result<Report> {
    let mut an = interp::Analysis::new(prog, STEP_BUDGET);
    an.run()?;

    // Widths of free names.
    let mut widths = Vec::new();
    let mut wdec = Vec::new();
    for (i, set) in an.frees.iter().enumerate() {
        let (name, line) = &prog.free_names[i];
        let w = if set.is_empty() {
            IntWidth::U8
        } else {
            if !set.bounded() {
                return Err(CompileError::new(*line, format!("cannot bound {name} (grows without limit in a loop); write an explicit type")));
            }
            width_for(prog, i, set.lo(), set.hi(), *line)?
        };
        widths.push(WidthReport { name: name.clone(), line: *line, width: w, note: if set.is_empty() { "never held a value".into() } else { format!("holds {}", set.describe()) } });
        wdec.push(Some(w));
    }

    // Sites.
    let mut site_checked = vec![false; prog.sites.len()];
    let mut sites = Vec::new();
    let mut check_errors: Vec<CompileError> = Vec::new();
    for s in &prog.sites {
        if !s.active {
            continue;
        }
        let st = &an.sites[s.id as usize];
        let complete = !an.incomplete[s.func as usize];
        let proven = complete && (st.proven || !st.reached) && s.free.is_none() || s.free.is_some();
        let (verdict, note) = if s.tier == Tier::Nocheck {
            (Verdict::Trusted, "nocheck".to_string())
        } else if proven {
            let note = if !st.reached && s.free.is_none() { "never reached".to_string() } else if s.free.is_some() { "free name: width chosen so this cannot fail".to_string() } else { format!("layer 2: {}", st.note) };
            (Verdict::Proven, note)
        } else {
            let mut note = if !complete { "inside a function whose analysis was cut short".to_string() } else { format!("layer 2: {}", st.note) };
            if st.certain {
                note.push_str("; always fails when reached");
            }
            if s.tier == Tier::Check {
                check_errors.push(CompileError::new(s.line, format!("`check` could not prove no {} in `{}` ({})", s.kind.text(), s.what, note)));
            }
            (Verdict::Checked, note)
        };
        site_checked[s.id as usize] = verdict == Verdict::Checked;
        sites.push(SiteReport { id: s.id, line: s.line, func: prog.funcs[s.func as usize].name.clone(), what: s.what.clone(), kind: s.kind.text(), verdict, note });
    }
    if let Some(e) = check_errors.into_iter().next() {
        return Err(e);
    }

    // Known values: pure expressions that were always the same.
    let mut known = HashMap::new();
    for (id, v) in &an.results {
        let Some(f) = an.expr_func.get(id) else { continue };
        if an.incomplete[*f as usize] {
            continue;
        }
        let k = match v {
            AVal::Int(s) => s.single().map(Known::Int),
            AVal::Bin(ABin::Known(f)) => Some(Known::Bin64(*f)),
            AVal::Bool(b) => b.single().map(Known::Bool),
            AVal::Str(s) => s.known.clone().map(Known::Str),
            _ => None,
        };
        if let Some(k) = k {
            known.insert(*id, k);
        }
    }
    // Bin constants must carry their width; the emitter reads the expression's
    // type, so store bin32 values as Bin32.
    fix_bin_widths(prog, &mut known);

    let decisions = Decisions { site_checked, widths: wdec, track_frees: false, known, whole: None };
    Ok(Report { decisions, sites, widths, whole: false, steps: an.steps })
}

fn fix_bin_widths(prog: &Program, known: &mut HashMap<u32, Known>) {
    let mut types: HashMap<u32, Ty> = HashMap::new();
    fn walk(e: &crate::ir::Expr, types: &mut HashMap<u32, Ty>) {
        use crate::ir::{EK, PieceIr};
        types.insert(e.id, e.ty.clone());
        match &e.kind {
            EK::Call(_, args) | EK::List(args) => args.iter().for_each(|a| walk(a, types)),
            EK::Pieces(ps) => ps.iter().for_each(|p| if let PieceIr::Value(v) = p { walk(v, types) }),
            EK::Neg(x, _) | EK::Not(x) | EK::Len(x) | EK::To(x, _) => walk(x, types),
            EK::Binary(_, a, b, ..) | EK::Index(a, b, _) | EK::Fill(a, b, _) => { walk(a, types); walk(b, types); }
            _ => {}
        }
    }
    fn block(b: &[crate::ir::Stmt], types: &mut HashMap<u32, Ty>) {
        use crate::ir::SK;
        for s in b {
            match &s.kind {
                SK::Let(_, e) | SK::Assign(_, e) | SK::Exit(e) | SK::CallStmt(e) | SK::Return(Some(e)) => walk(e, types),
                SK::AssignIndex(t, i, v, _) => { walk(t, types); walk(i, types); walk(v, types); }
                SK::Print { pieces, .. } => pieces.iter().for_each(|p| walk(p, types)),
                SK::If(brs, el) => {
                    for (c, b) in brs { walk(c, types); block(b, types); }
                    if let Some(b) = el { block(b, types); }
                }
                SK::Block(b) | SK::Loop(b) => block(b, types),
                SK::While(c, b) => { walk(c, types); block(b, types); }
                SK::ForRange { a, b, body, .. } => { walk(a, types); walk(b, types); block(body, types); }
                SK::ForList { list, body, .. } => { walk(list, types); block(body, types); }
                _ => {}
            }
        }
    }
    for f in &prog.funcs {
        block(&f.body, &mut types);
    }
    for (id, k) in known.iter_mut() {
        if let (Some(Ty::Bin(crate::ast::BinWidth::B32)), Known::Bin64(f)) = (types.get(id), k.clone()) {
            *k = Known::Bin32(f as f32);
        }
        // A free-typed or fixed integer constant is fine as is; a bin64 stays.
        if let (Some(Ty::Int(IntTy::Free(_))), Known::Int(_)) = (types.get(id), k.clone()) {}
    }
}

/// Render the report as text.
pub fn print_report(rep: &Report, types: bool) -> String {
    let mut out = String::new();
    if rep.whole {
        out.push_str(&format!("Precog ran the whole program at compile time ({} steps); the binary is its output.\n", rep.steps));
    } else {
        out.push_str(&format!("Precog analysed the program over its input bounds ({} steps).\n", rep.steps));
    }
    let mut sites = rep.sites.clone();
    sites.sort_by_key(|s| (s.line, s.id));
    for s in &sites {
        let v = match s.verdict { Verdict::Proven => "proven", Verdict::Checked => "checked", Verdict::Trusted => "trusted", Verdict::Ran => "ran" };
        out.push_str(&format!("line {:<4} {:<28} {:<18} {:<8} ({})\n", s.line, truncate(&s.what, 28), s.kind, v, s.note));
    }
    if types {
        for w in &rep.widths {
            out.push_str(&format!("{} (line {}): {} — {}\n", w.name, w.line, w.width.name(), w.note));
        }
    }
    let proven = sites.iter().filter(|s| matches!(s.verdict, Verdict::Proven | Verdict::Ran)).count();
    let checked = sites.iter().filter(|s| s.verdict == Verdict::Checked).count();
    let trusted = sites.iter().filter(|s| s.verdict == Verdict::Trusted).count();
    out.push_str(&format!("{} site(s): {proven} proven, {checked} checked at run time, {trusted} trusted\n", sites.len()));
    out
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n - 1).collect::<String>() + "…" }
}

#[allow(dead_code)]
fn unused(_: &ISet) {}
