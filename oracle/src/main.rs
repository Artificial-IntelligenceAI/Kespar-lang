//! `oracle [--steps N] [--trace-checks] file.kpls < input`
//! `oracle --types file.kpls`

use std::io::Write;
use std::process;

fn usage() -> ! {
    eprintln!("usage: oracle [--steps N] [--trace-checks] file.kpls < input\n       oracle --types file.kpls");
    process::exit(64);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut opts = oracle::Options::default();
    let mut types = false;
    let mut file: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--steps" => {
                i += 1;
                let n = args.get(i).and_then(|s| s.parse::<u64>().ok()).unwrap_or_else(|| usage());
                opts.steps = Some(n);
            }
            "--trace-checks" => opts.trace_checks = true,
            "--types" => types = true,
            s if s.starts_with("--") => usage(),
            s => {
                if file.is_some() {
                    usage();
                }
                file = Some(s.to_string());
            }
        }
        i += 1;
    }
    let path = file.unwrap_or_else(|| usage());
    let src = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("oracle: cannot read {}: {}", path, e);
            process::exit(64);
        }
    };

    if types {
        match oracle::types_listing(&src) {
            Ok(lines) => {
                let mut out = std::io::stdout().lock();
                for l in lines {
                    let _ = writeln!(out, "{}", l);
                }
                let _ = out.flush();
                process::exit(0);
            }
            Err(e) => {
                eprintln!("{}", e);
                process::exit(3);
            }
        }
    }

    // deep recursion in the program is deep recursion in the interpreter
    let child = std::thread::Builder::new()
        .stack_size(1 << 30)
        .spawn(move || {
            let stdin = std::io::stdin();
            let mut input = stdin.lock();
            oracle::run(&src, &mut input, &opts)
        })
        .expect("spawn interpreter thread");
    let outcome = child.join().expect("interpreter thread panicked");

    let mut out = std::io::stdout().lock();
    let _ = out.write_all(&outcome.stdout);
    for l in &outcome.trace {
        let _ = writeln!(out, "{}", l);
    }
    let _ = out.flush();
    if let Some(msg) = &outcome.stderr {
        eprintln!("{}", msg);
    }
    process::exit(outcome.exit);
}
