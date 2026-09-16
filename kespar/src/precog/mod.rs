//! Precog: what the compiler knows about the program before it runs.
//! Placeholder until the real layers land: every site keeps its check,
//! every free name is int64.

use crate::ast::IntWidth;
use crate::diag::Result;
use crate::emit::Decisions;
use crate::ir::Program;

pub struct Report {
    pub decisions: Decisions,
}

pub fn analyse(prog: &Program) -> Result<Report> {
    let decisions = Decisions {
        site_checked: vec![true; prog.sites.len()],
        widths: vec![IntWidth::I64; prog.free_names.len()],
        known: Default::default(),
        whole: None,
    };
    Ok(Report { decisions })
}
