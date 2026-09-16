use std::process::exit;

fn usage() -> ! {
    eprintln!("usage: kespar check|build|run [--steps N] [--all-checks] [--report] [--types] [--count] file.kpls");
    exit(64)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        usage();
    }
    let cmd = args[0].clone();
    let mut steps: Option<u64> = None;
    let mut all_checks = false;
    let mut report = false;
    let mut types = false;
    let mut count = false;
    let mut file = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--steps" => {
                i += 1;
                steps = Some(args.get(i).and_then(|s| s.parse().ok()).unwrap_or_else(|| usage()));
            }
            "--all-checks" => all_checks = true,
            "--report" => report = true,
            "--types" => types = true,
            "--count" => count = true,
            s if s.starts_with("--") => usage(),
            s => file = Some(s.to_string()),
        }
        i += 1;
    }
    let Some(file) = file else { usage() };
    let src = match std::fs::read_to_string(&file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("kespar: cannot read {file}: {e}");
            exit(66);
        }
    };
    let prog = match kespar::front(&src) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            exit(3);
        }
    };
    let rep = match kespar::precog::analyse(&prog) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            exit(3);
        }
    };
    let mut dec = rep.decisions.clone();
    if all_checks {
        dec.site_checked = prog.sites.iter().map(|s| s.active && s.tier != kespar::ast::Tier::Nocheck).collect();
        dec.known.clear();
        dec.whole = None;
    }
    if report || types || cmd == "check" {
        print!("{}", kespar::precog::print_report(&rep, types));
    }
    match cmd.as_str() {
        "check" => {}
        "build" => {
            let module = kespar::emit::emit(&prog, &dec);
            let out = file.replace(".kpls", "") + ".kpbc";
            std::fs::write(&out, format!("{module:#?}")).unwrap();
        }
        "run" => {
            let module = kespar::emit::emit(&prog, &dec);
            if count {
                let stdin = std::io::stdin();
                let started = std::time::Instant::now();
                let r = kespar::vm::run(&module, Box::new(stdin.lock()), steps);
                let ran = started.elapsed();
                use std::io::Write;
                let mut so = std::io::stdout().lock();
                let _ = so.write_all(&r.stdout);
                let _ = so.flush();
                if let Some(l) = r.outcome.stderr_line() {
                    eprintln!("{l}");
                }
                let checks = module.funcs.iter().flat_map(|f| f.ops.iter()).filter(|op| matches!(op, kespar::vm::bytecode::Op::IntOp { overflow: Some(_), .. } | kespar::vm::bytecode::Op::IntOp { second: Some(_), .. } | kespar::vm::bytecode::Op::IntNeg { overflow: Some(_), .. } | kespar::vm::bytecode::Op::Index { site: Some(_), .. } | kespar::vm::bytecode::Op::StoreIndex { site: Some(_), .. } | kespar::vm::bytecode::Op::IntToInt { overflow: Some(_), .. } | kespar::vm::bytecode::Op::BinToInt { overflow: Some(_), .. } | kespar::vm::bytecode::Op::Fill { site: Some(_), .. })).count();
                let ops: usize = module.funcs.iter().map(|f| f.ops.len()).sum();
                eprintln!("kespar: {} steps executed in {:.3}s; {} instructions emitted, {} of them checked", r.steps, ran.as_secs_f64(), ops, checks);
                exit(r.outcome.exit_code());
            }
            exit(kespar::vm::run_process(&module, steps));
        }
        _ => usage(),
    }
}
