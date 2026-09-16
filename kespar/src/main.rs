use std::process::exit;

fn usage() -> ! {
    eprintln!("usage: kespar check|build|run [--steps N] [--all-checks] [--report] [--types] file.kpls");
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
            exit(kespar::vm::run_process(&module, steps));
        }
        _ => usage(),
    }
}
