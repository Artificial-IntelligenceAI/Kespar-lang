//! Every examples/*.kpls must print its .out (stdin from .in if present),
//! exit with .exit (default 0), and — when it stops — say .err on stderr.

use std::fs;
use std::path::Path;

#[test]
fn examples() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples");
    let mut names: Vec<_> = fs::read_dir(&dir).unwrap().filter_map(|e| {
        let p = e.unwrap().path();
        (p.extension().map_or(false, |x| x == "kpls")).then(|| p)
    }).collect();
    names.sort();
    assert!(!names.is_empty());
    let mut failures = Vec::new();
    for path in names {
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        let src = fs::read_to_string(&path).unwrap();
        let input = fs::read(dir.join(format!("{stem}.in"))).unwrap_or_default();
        let want_out = fs::read(dir.join(format!("{stem}.out"))).unwrap_or_default();
        let want_exit: i32 = fs::read_to_string(dir.join(format!("{stem}.exit"))).map(|s| s.trim().parse().unwrap()).unwrap_or(0);
        let want_err = fs::read_to_string(dir.join(format!("{stem}.err"))).ok().map(|s| s.trim().to_string());
        let (out, exit, err) = match kespar::front(&src).and_then(|p| kespar::precog::analyse(&p).map(|r| (p, r))) {
            Err(e) => (Vec::new(), 3, Some(e.to_string())),
            Ok((prog, rep)) => {
                let module = kespar::emit::emit(&prog, &rep.decisions);
                let r = kespar::vm::run(&module, Box::new(std::io::Cursor::new(input)), Some(50_000_000));
                (r.stdout, r.outcome.exit_code(), r.outcome.stderr_line())
            }
        };
        if out != want_out || exit != want_exit || (want_err.is_some() && err != want_err) {
            failures.push(format!("{stem}: exit {exit} (want {want_exit}), stderr {err:?} (want {want_err:?})\n--- got ---\n{}\n--- want ---\n{}", String::from_utf8_lossy(&out), String::from_utf8_lossy(&want_out)));
        }
    }
    if !failures.is_empty() {
        panic!("{} example(s) failed:\n{}", failures.len(), failures.join("\n"));
    }
}
