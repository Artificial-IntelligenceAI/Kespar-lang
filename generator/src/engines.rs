//! The engines a case is run through, and what each says.


/// What an engine said about one program on one input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
    /// The engine refused the program: the message.
    pub compile_error: Option<String>,
    pub stdout: String,
    pub exit: i32,
    /// The single stderr line a stop writes, if any.
    pub stderr: String,
    /// Check sites that fired: (line, kind).
    pub fired: Vec<(u32, String)>,
    /// Free names and the range they held: (name, min, max).
    pub held: Vec<(String, i128, i128)>,
}

impl Answer {
    pub fn budget(&self) -> bool {
        self.exit == 4
    }
    pub fn summary(&self) -> String {
        match &self.compile_error {
            Some(e) => format!("compile error: {e}"),
            None => format!("exit {} stdout {:?} stderr {:?}", self.exit, self.stdout, self.stderr),
        }
    }
}

/// What Precog claimed about a program.
#[derive(Debug, Clone, Default)]
pub struct Claims {
    /// (line, kind) of every site whose check was removed as proven.
    pub proven: Vec<(u32, String)>,
    /// (line, kind) of every site whose check stayed.
    pub checked: Vec<(u32, String)>,
    /// Free names: (name, line, width min, width max).
    pub widths: Vec<(String, u32, i128, i128)>,
    /// A `check` tier refusal or a Precog refusal (not a checker error).
    pub refused: Option<String>,
    /// Layer 3 said a site fails for these inputs (line, kind, description).
    pub counterexamples: Vec<(u32, String, String)>,
}

pub struct Kespar;

impl Kespar {
    /// Compile once: the claims, and a closure-free handle to run on inputs.
    pub fn compile(src: &str, all_checks: bool) -> Result<(kespar::vm::bytecode::Module, Claims), Answer> {
        match std::panic::catch_unwind(|| Self::compile_inner(src, all_checks)) {
            Ok(r) => r,
            Err(p) => {
                let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                Err(Answer { compile_error: Some(format!("KESPAR PANICKED: {msg}")), stdout: String::new(), exit: -2, stderr: String::new(), fired: vec![], held: vec![] })
            }
        }
    }

    fn compile_inner(src: &str, all_checks: bool) -> Result<(kespar::vm::bytecode::Module, Claims), Answer> {
        let prog = match kespar::front(src) {
            Ok(p) => p,
            Err(e) => return Err(Answer { compile_error: Some(e.to_string()), stdout: String::new(), exit: 3, stderr: e.to_string(), fired: vec![], held: vec![] }),
        };
        let rep = match kespar::precog::analyse(&prog) {
            Ok(r) => r,
            Err(e) => {
                let msg = e.to_string();
                let mut claims = Claims::default();
                claims.refused = Some(msg.clone());
                // A Precog refusal is not a checker error; the oracle will still run the program.
                return Err(Answer { compile_error: Some(msg.clone()), stdout: String::new(), exit: 3, stderr: msg, fired: vec![], held: vec![] });
            }
        };
        let mut claims = Claims::default();
        for s in &rep.sites {
            match s.verdict {
                kespar::precog::Verdict::Proven | kespar::precog::Verdict::Ran => claims.proven.push((s.line, s.kind.to_string())),
                kespar::precog::Verdict::Checked => {
                    claims.checked.push((s.line, s.kind.to_string()));
                    if let Some(rest) = s.note.strip_prefix("layer 3: fails for ") {
                        claims.counterexamples.push((s.line, s.kind.to_string(), rest.to_string()));
                    }
                }
                kespar::precog::Verdict::Trusted => {}
            }
        }
        for w in &rep.widths {
            claims.widths.push((w.name.clone(), w.line, w.width.min(), w.width.max()));
        }
        let mut dec = rep.decisions.clone();
        if all_checks {
            dec.site_checked = prog.sites.iter().map(|s| s.active && s.tier != kespar::ast::Tier::Nocheck).collect();
            dec.known.clear();
            dec.whole = None;
        }
        Ok((kespar::emit::emit(&prog, &dec), claims))
    }

    pub fn run(module: &kespar::vm::bytecode::Module, input: &str, steps: u64) -> Answer {
        match std::panic::catch_unwind(|| Self::run_inner(module, input, steps)) {
            Ok(a) => a,
            Err(p) => {
                let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                Answer { compile_error: None, stdout: String::new(), exit: -2, stderr: format!("KESPAR VM PANICKED: {msg}"), fired: vec![], held: vec![] }
            }
        }
    }

    fn run_inner(module: &kespar::vm::bytecode::Module, input: &str, steps: u64) -> Answer {
        let r = kespar::vm::run(module, Box::new(std::io::Cursor::new(input.as_bytes().to_vec())), Some(steps));
        Answer {
            compile_error: None,
            stdout: String::from_utf8_lossy(&r.stdout).into_owned(),
            exit: r.outcome.exit_code(),
            stderr: r.outcome.stderr_line().unwrap_or_default(),
            fired: match &r.outcome {
                kespar::vm::Outcome::Trap { kind, line, .. } => vec![(*line, kind.text().to_string())],
                _ => vec![],
            },
            held: vec![],
        }
    }
}

/// The reference interpreter, linked in (`design/oracle.md`): compiled once
/// per program, run per input.
pub struct Oracle {
    checked: Result<oracle::check::Checked, String>,
}

impl Oracle {
    pub fn compile(src: &str) -> Oracle {
        let checked = match std::panic::catch_unwind(|| oracle::compile(src)) {
            Ok(Ok(c)) => Ok(c),
            Ok(Err(e)) => Err(e.to_string()),
            Err(p) => {
                let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                Err(format!("ORACLE PANICKED: {msg}"))
            }
        };
        Oracle { checked }
    }

    pub fn compile_error(&self) -> Option<&str> {
        self.checked.as_ref().err().map(|s| s.as_str())
    }

    pub fn run(&self, input: &str, steps: u64) -> Answer {
        let checked = match &self.checked {
            Ok(c) => c,
            Err(e) => return Answer { compile_error: Some(e.clone()), stdout: String::new(), exit: 3, stderr: e.clone(), fired: vec![], held: vec![] },
        };
        let opts = oracle::Options { steps: Some(steps), trace_checks: true };
        let out = match std::panic::catch_unwind(|| {
            let mut cursor = std::io::Cursor::new(input.as_bytes().to_vec());
            oracle::interp::run(checked, &mut cursor, &opts)
        }) {
            Ok(o) => o,
            Err(p) => {
                let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                return Answer { compile_error: None, stdout: String::new(), exit: -2, stderr: format!("ORACLE PANICKED: {msg}"), fired: vec![], held: vec![] };
            }
        };
        let mut fired = Vec::new();
        let mut held = Vec::new();
        for t in &out.trace {
            let t = t.trim_end();
            if let Some(rest) = t.strip_prefix("site ") {
                let (ln, rest) = rest.split_once(' ').unwrap_or((rest, ""));
                if let Some(kind) = rest.strip_suffix(" fired") {
                    if let Ok(l) = ln.parse() {
                        fired.push((l, kind.to_string()));
                    }
                }
            } else if let Some(rest) = t.strip_prefix("name ") {
                if let Some((name, rest)) = rest.rsplit_once("' min ") {
                    let name = name.trim_start_matches('\'').to_string();
                    if let Some((mn, mx)) = rest.split_once(" max ") {
                        if let (Ok(a), Ok(b)) = (mn.trim().parse(), mx.trim().parse()) {
                            held.push((name, a, b));
                        }
                    }
                }
            }
        }
        Answer { compile_error: None, stdout: String::from_utf8_lossy(&out.stdout).into_owned(), exit: out.exit, stderr: out.stderr.unwrap_or_default(), fired, held }
    }
}
