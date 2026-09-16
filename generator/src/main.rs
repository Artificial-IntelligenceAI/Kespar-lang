//! The differential runner: write a program, run it through every engine on
//! every input, compare what they said, attack every claim Precog made.
//! One worker per core, one shared counter, everything reproducible from a
//! seed. See design/generator.md and design/oracle.md.

mod engines;
mod gen;
mod rng;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use engines::{Answer, Claims, Kespar, Oracle};

struct Settings {
    cases: u64,
    first_seed: u64,
    size: u32,
    jobs: usize,
    keep_going: bool,
    steps: u64,
    oracle: bool,
    workspace: PathBuf,
    /// Enumerate every input when the read domains multiply to at most this.
    exhaust: u64,
    no_shrink: bool,
}

#[derive(Debug, Clone)]
enum Kind {
    /// The generator wrote a program an engine refused.
    Broke { engine: String, message: String },
    /// Engines disagreed on an input.
    Disagree { input: String, answers: Vec<(String, Answer)>, odd: Option<String> },
    /// A site Precog proved fired in the reference interpreter.
    Claim { input: String, line: u32, kind: String },
    /// A free name held a value outside the width Precog chose.
    Width { input: String, name: String, held: (i128, i128), width: (i128, i128) },
    /// Layer 3's counterexample did not fail in the reference interpreter.
    Counterexample { line: u32, kind: String, said: String },
}

#[derive(Debug, Clone)]
struct Finding {
    seed: u64,
    program: String,
    kind: Kind,
}

#[derive(Default)]
struct Stats {
    cases: u64,
    inputs: u64,
    refused: u64,
    budget: u64,
    proven_sites: u64,
    checked_sites: u64,
    exhaustive: u64,
}

fn usage() -> ! {
    eprintln!("usage: generator run --cases N [--seed S] [--size K] [--jobs J] [--keep-going] [--steps N] [--no-oracle] [--exhaust N] [--no-shrink]");
    eprintln!("       generator one --seed S [--size K]");
    eprintln!("       generator replay --seed S [--size K] [--no-oracle]");
    std::process::exit(64)
}

fn main() {
    // Panics inside an engine are findings, not noise.
    if std::env::var("GENERATOR_PANICS").is_err() {
        std::panic::set_hook(Box::new(|_| {}));
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        usage();
    }
    let mut s = Settings { cases: 1000, first_seed: 1, size: 3, jobs: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1), keep_going: false, steps: 2_000_000, oracle: true, workspace: PathBuf::from("work"), exhaust: 4096, no_shrink: false };
    let cmd = args[0].clone();
    let mut i = 1;
    let val = |i: &mut usize| -> String { *i += 1; args.get(*i).cloned().unwrap_or_else(|| usage()) };
    while i < args.len() {
        match args[i].as_str() {
            "--cases" => s.cases = val(&mut i).parse().unwrap_or_else(|_| usage()),
            "--seed" => s.first_seed = val(&mut i).parse().unwrap_or_else(|_| usage()),
            "--size" => s.size = val(&mut i).parse().unwrap_or_else(|_| usage()),
            "--jobs" => s.jobs = val(&mut i).parse().unwrap_or_else(|_| usage()),
            "--steps" => s.steps = val(&mut i).parse().unwrap_or_else(|_| usage()),
            "--no-oracle" => s.oracle = false,
            "--exhaust" => s.exhaust = val(&mut i).parse().unwrap_or_else(|_| usage()),
            "--keep-going" => s.keep_going = true,
            "--no-shrink" => s.no_shrink = true,
            _ => usage(),
        }
        i += 1;
    }
    match cmd.as_str() {
        "one" => {
            let c = gen::case(s.first_seed, s.size);
            print!("{}", c.program);
            for (k, inp) in c.inputs.iter().enumerate() {
                println!("--- input {k} ---");
                print!("{inp}");
            }
        }
        "replay" => {
            let _ = std::fs::create_dir_all(&s.workspace);
            let mut stats = Stats::default();
            match run_case(&s, s.first_seed, s.size, &s.workspace.join("replay"), &mut stats, true) {
                Some(f) => print_finding(&f),
                None => println!("seed {}: every engine agreed on {} input(s)", s.first_seed, stats.inputs),
            }
        }
        "run" => run(&s),
        _ => usage(),
    }
}

fn run(s: &Settings) {
    let _ = std::fs::create_dir_all(&s.workspace);
    let next = AtomicU64::new(0);
    let stop = AtomicBool::new(false);
    let findings: Mutex<Vec<Finding>> = Mutex::new(Vec::new());
    let totals: Mutex<Stats> = Mutex::new(Stats::default());
    let start = Instant::now();
    let done = AtomicU64::new(0);
    std::thread::scope(|scope| {
        for w in 0..s.jobs {
            let (next, stop, findings, totals, done) = (&next, &stop, &findings, &totals, &done);
            let dir = s.workspace.join(w.to_string());
            let _ = std::fs::create_dir_all(&dir);
            scope.spawn(move || {
                background_priority();
                let mut stats = Stats::default();
                loop {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    if k >= s.cases {
                        break;
                    }
                    let seed = s.first_seed + k;
                    if let Some(f) = run_case(s, seed, s.size, &dir, &mut stats, false) {
                        let f = if s.no_shrink { f } else { shrink(s, f, &dir) };
                        findings.lock().unwrap().push(f);
                        if !s.keep_going {
                            stop.store(true, Ordering::Relaxed);
                        }
                    }
                    done.fetch_add(1, Ordering::Relaxed);
                }
                let mut t = totals.lock().unwrap();
                t.cases += stats.cases;
                t.inputs += stats.inputs;
                t.refused += stats.refused;
                t.budget += stats.budget;
                t.proven_sites += stats.proven_sites;
                t.checked_sites += stats.checked_sites;
                t.exhaustive += stats.exhaustive;
            });
        }
        // Progress.
        scope.spawn(|| {
            let mut last = 0;
            while !stop.load(Ordering::Relaxed) && done.load(Ordering::Relaxed) < s.cases {
                std::thread::sleep(std::time::Duration::from_millis(500));
                let d = done.load(Ordering::Relaxed);
                if d != last {
                    let secs = start.elapsed().as_secs_f64();
                    eprint!("\r{d}/{} cases, {:.0}/s, {} finding(s)   ", s.cases, d as f64 / secs, findings.lock().unwrap().len());
                    last = d;
                }
            }
        });
    });
    eprintln!();
    let t = totals.lock().unwrap();
    let secs = start.elapsed().as_secs_f64();
    println!("{} cases, {} inputs, in {secs:.1}s on {} worker(s) ({:.0} cases/s)", t.cases, t.inputs, s.jobs, t.cases as f64 / secs.max(0.001));
    println!("sites: {} proven, {} checked; {} Precog refusals; {} exhaustive case(s); {} budget stops", t.proven_sites, t.checked_sites, t.refused, t.exhaustive, t.budget);
    let fs = findings.lock().unwrap();
    for f in fs.iter() {
        println!();
        print_finding(f);
    }
    if !fs.is_empty() {
        std::process::exit(1);
    }
}

fn print_finding(f: &Finding) {
    println!("=== seed {} ===", f.seed);
    print!("{}", f.program);
    match &f.kind {
        Kind::Broke { engine, message } => println!("--- {engine} refused the program: {message}"),
        Kind::Disagree { input, answers, odd } => {
            println!("--- input:\n{input}--- answers:");
            for (name, a) in answers {
                println!("  {name:<10} {}", a.summary());
            }
            match odd {
                Some(o) => println!("--- {o} is the one out of step"),
                None => println!("--- no single engine stands apart"),
            }
        }
        Kind::Claim { input, line, kind } => println!("--- CLAIM BROKEN: Precog proved no {kind} at line {line}, but it fired on input:\n{input}"),
        Kind::Width { input, name, held, width } => println!("--- WIDTH BROKEN: {name} held {}..{} but Precog chose a width holding {}..{}; input:\n{input}", held.0, held.1, width.0, width.1),
        Kind::Counterexample { line, kind, said } => println!("--- counterexample did not reproduce: layer 3 said {kind} at line {line} fails for {said}"),
    }
}

/// The oracle's compile error for a program the generator wrote is a
/// finding unless it mirrors a Precog refusal; Kespar's checker errors are
/// always findings. Precog's own refusals are counted, not reported.
fn precog_refusal(msg: &str) -> bool {
    !msg.contains("PANICKED") && (msg.contains("no integer type holds") || msg.contains("cannot bound") || msg.contains("exceeded") || msg.contains("`check`") || msg.contains("always stops"))
}

fn run_case(s: &Settings, seed: u64, size: u32, dir: &Path, stats: &mut Stats, verbose: bool) -> Option<Finding> {
    let case = gen::case(seed, size);
    stats.cases += 1;
    // The program is written out only when a finding needs looking at (or on replay):
    // a file per case is thousands of files per minute for the indexer to chew on.
    let src_path = dir.join(format!("case_{seed}.kpls"));
    if verbose {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(&src_path, &case.program);
    }
    let program = case.program.clone();
    let finding = |kind: Kind| Some(Finding { seed, program: program.clone(), kind });

    // Compile with Precog and with every check kept.
    let precog = Kespar::compile(&case.program, false);
    let allchecks = Kespar::compile(&case.program, true);
    let oracle = if s.oracle { Some(Oracle::compile(&case.program)) } else { None };
    if let Some(o) = &oracle {
        if let Some(e) = o.compile_error() {
            if e.contains("PANICKED") {
                return finding(Kind::Broke { engine: "oracle".into(), message: e.to_string() });
            }
        }
    }

    let mut inputs = case.inputs.clone();
    if let Some(all) = exhaustive_inputs(&case.reads, s.exhaust) {
        stats.exhaustive += 1;
        inputs = all;
    }

    // A refusal by Precog (as opposed to the checker) stops the comparison
    // unless it is the "always stops" kind, which the oracle must confirm.
    let (module, claims) = match precog {
        Ok(x) => x,
        Err(ans) => {
            let msg = ans.compile_error.clone().unwrap_or_default();
            if let Some(o) = &oracle {
                let oa = o.run(inputs.first().map(|s| s.as_str()).unwrap_or(""), s.steps);
                if msg.contains("always stops with ") {
                    // "error: this program always stops with <kind> ... at line N"
                    let kind = msg.split("always stops with ").nth(1).unwrap_or("").split(" (").next().unwrap_or("").to_string();
                    let line = msg.rsplit("at line ").next().and_then(|l| l.trim().parse::<u32>().ok()).unwrap_or(0);
                    stats.inputs += 1;
                    if oa.exit == 1 && oa.stderr == format!("kespar: {kind} at line {line}") {
                        return None;
                    }
                    if oa.budget() {
                        stats.budget += 1;
                        return None;
                    }
                    return finding(Kind::Disagree { input: inputs[0].clone(), answers: vec![("precog".into(), ans), ("oracle".into(), oa)], odd: None });
                }
                if precog_refusal(&msg) {
                    stats.refused += 1;
                    if oa.exit == 3 {
                        return finding(Kind::Broke { engine: "oracle".into(), message: oa.stderr });
                    }
                    return None;
                }
                // A checker error: the oracle must reject too, or the generator is wrong.
                if oa.exit == 3 {
                    return finding(Kind::Broke { engine: "both".into(), message: format!("kespar: {msg}; oracle: {}", oa.stderr) });
                }
                return finding(Kind::Broke { engine: "kespar".into(), message: msg });
            }
            if precog_refusal(&msg) {
                stats.refused += 1;
                return None;
            }
            return finding(Kind::Broke { engine: "kespar".into(), message: msg });
        }
    };
    let (module_all, _) = match allchecks {
        Ok(x) => x,
        Err(ans) => return finding(Kind::Broke { engine: "kespar (all checks)".into(), message: ans.compile_error.unwrap_or_default() }),
    };
    stats.proven_sites += claims.proven.len() as u64;
    stats.checked_sites += claims.checked.len() as u64;

    let mut oracle_fired: Vec<(u32, String)> = Vec::new();
    let mut oracle_held: Vec<(String, i128, i128)> = Vec::new();
    for input in &inputs {
        stats.inputs += 1;
        let a1 = Kespar::run(&module, input, s.steps);
        let a2 = Kespar::run(&module_all, input, s.steps);
        let mut answers = vec![("precog".to_string(), a1), ("allchecks".to_string(), a2)];
        if let Some(o) = &oracle {
            let oa = o.run(input, s.steps);
            if oa.compile_error.is_some() {
                return finding(Kind::Broke { engine: "oracle".into(), message: oa.stderr });
            }
            oracle_fired.extend(oa.fired.iter().cloned());
            oracle_held.extend(oa.held.iter().cloned());
            answers.push(("oracle".to_string(), oa));
        }
        if answers.iter().any(|(_, a)| a.budget()) {
            stats.budget += 1;
            continue;
        }
        if verbose {
            for (n, a) in &answers {
                println!("[{n}] {}", a.summary());
            }
        }
        if let Some(odd) = odd_one_out(&answers) {
            return finding(Kind::Disagree { input: input.clone(), answers, odd });
        }
    }

    // Claims.
    if oracle.is_some() {
        for (line, kind) in &claims.proven {
            let fired = oracle_fired.iter().any(|(l, k)| l == line && k == kind);
            let also_checked = claims.checked.iter().any(|(l, k)| l == line && k == kind);
            if fired && !also_checked {
                return finding(Kind::Claim { input: inputs.join("\n---\n"), line: *line, kind: kind.clone() });
            }
        }
        for (name, _line, wmin, wmax) in &claims.widths {
            for (n, lo, hi) in &oracle_held {
                if n == name && (lo < wmin || hi > wmax) {
                    return finding(Kind::Width { input: inputs.join("\n---\n"), name: name.clone(), held: (*lo, *hi), width: (*wmin, *wmax) });
                }
            }
        }
        // A counterexample must reproduce: build its input from the description.
        for (line, kind, said) in &claims.counterexamples {
            if let Some(input) = input_from_counterexample(&case.reads, said) {
                let oa = oracle.as_ref().unwrap().run(&input, s.steps);
                if oa.budget() {
                    continue;
                }
                let hit = oa.exit == 1 && oa.stderr == format!("kespar: {kind} at line {line}");
                if !hit {
                    return finding(Kind::Counterexample { line: *line, kind: kind.clone(), said: said.clone() });
                }
            }
        }
    }
    None
}

/// Which engine stands apart, if exactly one does (Xag's rule).
fn odd_one_out(answers: &[(String, Answer)]) -> Option<Option<String>> {
    let same = |a: &Answer, b: &Answer| a.stdout == b.stdout && a.exit == b.exit && a.stderr == b.stderr;
    let n = answers.len();
    let all_agree = answers.iter().all(|(_, a)| same(a, &answers[0].1));
    if all_agree {
        return None;
    }
    if n < 3 {
        return Some(None);
    }
    for i in 0..n {
        let others_agree = (0..n).filter(|j| *j != i).all(|j| (0..n).filter(|k| *k != i).all(|k| same(&answers[j].1, &answers[k].1)));
        if others_agree {
            return Some(Some(answers[i].0.clone()));
        }
    }
    Some(None)
}

/// Every input, when the scalar read domains are small enough.
fn exhaustive_inputs(reads: &[gen::Read], cap: u64) -> Option<Vec<String>> {
    if reads.is_empty() {
        return None;
    }
    let mut domains: Vec<Vec<String>> = Vec::new();
    let mut total: u64 = 1;
    for r in reads {
        let d: Vec<String> = match &r.ty {
            gen::Ty::Int(_) => {
                let (lo, hi) = r.value?;
                if hi - lo + 1 > cap as i128 {
                    return None;
                }
                (lo..=hi).map(|v| v.to_string()).collect()
            }
            gen::Ty::Bool => vec!["true".into(), "false".into()],
            _ => return None,
        };
        total = total.checked_mul(d.len() as u64)?;
        if total > cap {
            return None;
        }
        domains.push(d);
    }
    let mut out = vec![String::new()];
    for d in domains {
        let mut next = Vec::with_capacity(out.len() * d.len());
        for prefix in &out {
            for v in &d {
                next.push(format!("{prefix}{v}\n"));
            }
        }
        out = next;
    }
    Some(out)
}

/// Turn layer 3's "'n'=5, 'k'=-1" into input lines (scalars only).
fn input_from_counterexample(reads: &[gen::Read], said: &str) -> Option<String> {
    let mut lines = Vec::new();
    for r in reads {
        let key = format!("'{}'=", r.name);
        let v = said.split(", ").find_map(|p| p.strip_prefix(&key))?;
        match &r.ty {
            gen::Ty::Int(_) | gen::Ty::Bool => lines.push(v.to_string()),
            _ => return None,
        }
    }
    Some(lines.join("\n") + "\n")
}

/// Cut the program down while the finding survives.
fn shrink(s: &Settings, f: Finding, dir: &Path) -> Finding {
    let same_kind = |a: &Kind, b: &Kind| std::mem::discriminant(a) == std::mem::discriminant(b);
    let mut best = f.clone();
    let mut progress = true;
    let mut rounds = 0;
    while progress && rounds < 20 {
        progress = false;
        rounds += 1;
        let lines: Vec<String> = best.program.lines().map(|l| l.to_string()).collect();
        let mut i = 0;
        while i < lines.len() {
            let l = lines[i].trim();
            // Candidates: one simple statement, or a whole block.
            let end = if l.ends_with('{') && !l.starts_with("MAIN") && !l.starts_with("func") {
                match matching_close(&lines, i) { Some(e) => e, None => { i += 1; continue; } }
            } else if l.ends_with(';') && !l.contains("std::read") && !l.starts_with("return") {
                i
            } else {
                i += 1;
                continue;
            };
            let mut cand: Vec<String> = lines[..i].to_vec();
            cand.extend_from_slice(&lines[end + 1..]);
            let program = cand.join("\n") + "\n";
            if let Some(nf) = run_source(s, &program, best.seed, dir) {
                if same_kind(&nf.kind, &best.kind) {
                    best = nf;
                    progress = true;
                    break;
                }
            }
            i += 1;
        }
    }
    best
}

fn matching_close(lines: &[String], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (k, l) in lines.iter().enumerate().skip(open) {
        let t = l.trim();
        if t.ends_with('{') { depth += 1; }
        if t.starts_with('}') { depth -= 1; if t.ends_with('{') { depth += 1; } }
        if depth == 0 && k > open {
            return Some(k);
        }
        if t.starts_with('}') && t.ends_with('{') && depth == 0 {
            return None;
        }
    }
    None
}

/// Run a hand-edited program through the same comparison (for shrinking).
fn run_source(s: &Settings, program: &str, seed: u64, dir: &Path) -> Option<Finding> {
    // Reuse the seed's inputs: the reads are untouched by shrinking.
    let case = gen::case(seed, s.size);
    let _ = dir;
    let oracle = if s.oracle { Some(Oracle::compile(program)) } else { None };
    let precog = Kespar::compile(program, false);
    let allchecks = Kespar::compile(program, true);
    let finding = |kind: Kind| Some(Finding { seed, program: program.to_string(), kind });
    let (module, claims): (kespar::vm::bytecode::Module, Claims) = match precog {
        Ok(x) => x,
        Err(ans) => {
            let msg = ans.compile_error.clone().unwrap_or_default();
            if precog_refusal(&msg) {
                return None;
            }
            return finding(Kind::Broke { engine: "kespar".into(), message: msg });
        }
    };
    let (module_all, _) = allchecks.ok()?;
    let mut oracle_fired = Vec::new();
    for input in &case.inputs {
        let a1 = Kespar::run(&module, input, s.steps);
        let a2 = Kespar::run(&module_all, input, s.steps);
        let mut answers = vec![("precog".to_string(), a1), ("allchecks".to_string(), a2)];
        if let Some(o) = &oracle {
            let oa = o.run(input, s.steps);
            if oa.compile_error.is_some() {
                return finding(Kind::Broke { engine: "oracle".into(), message: oa.stderr });
            }
            oracle_fired.extend(oa.fired.iter().cloned());
            answers.push(("oracle".to_string(), oa));
        }
        if answers.iter().any(|(_, a)| a.budget()) {
            continue;
        }
        if let Some(odd) = odd_one_out(&answers) {
            return finding(Kind::Disagree { input: input.clone(), answers, odd });
        }
    }
    for (line, kind) in &claims.proven {
        if oracle_fired.iter().any(|(l, k)| l == line && k == kind) && !claims.checked.iter().any(|(l, k)| l == line && k == kind) {
            return finding(Kind::Claim { input: case.inputs.join("\n---\n"), line: *line, kind: kind.clone() });
        }
    }
    None
}

/// Use every core, but as background work: the moment another app wants a
/// core, the scheduler gives it away. macOS QoS "background" also lowers
/// I/O priority; the plain `nice` covers other Unixes.
fn background_priority() {
    #[cfg(target_os = "macos")]
    {
        extern "C" {
            fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
        }
        const QOS_CLASS_BACKGROUND: u32 = 0x09;
        unsafe {
            pthread_set_qos_class_self_np(QOS_CLASS_BACKGROUND, 0);
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        extern "C" {
            fn setpriority(which: i32, who: u32, prio: i32) -> i32;
        }
        unsafe {
            setpriority(0, 0, 19);
        }
    }
}
