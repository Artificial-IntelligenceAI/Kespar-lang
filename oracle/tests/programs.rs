//! Runs every `tests/cases/*.kpls` through the `oracle` binary and compares
//! stdout (`.out`), exit code (`.exit`, default 0) and stderr (`.err`, a
//! prefix). `.in` is stdin; `.args` holds extra command-line flags.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn read_or(path: &Path, default: &str) -> String {
    fs::read_to_string(path).unwrap_or_else(|_| default.to_string())
}

fn run_case(kpls: &Path) -> Result<(), String> {
    let stem = kpls.with_extension("");
    let expected_out = read_or(&stem.with_extension("out"), "");
    let expected_exit: i32 = read_or(&stem.with_extension("exit"), "0").trim().parse().unwrap();
    let expected_err = fs::read_to_string(stem.with_extension("err")).ok();
    let input = fs::read(stem.with_extension("in")).unwrap_or_default();
    let args: Vec<String> = read_or(&stem.with_extension("args"), "")
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();

    let mut child = Command::new(env!("CARGO_BIN_EXE_oracle"))
        .args(&args)
        .arg(kpls)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn: {}", e))?;
    {
        let mut stdin = child.stdin.take().unwrap();
        let _ = stdin.write_all(&input);
    }
    let out = child.wait_with_output().map_err(|e| format!("wait: {}", e))?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let code = out.status.code().unwrap_or(-1);

    let mut problems = Vec::new();
    if stdout != expected_out {
        problems.push(format!("stdout differs\n--- expected ---\n{}\n--- actual ---\n{}", expected_out, stdout));
    }
    if code != expected_exit {
        problems.push(format!("exit {} expected, got {} (stderr: {})", expected_exit, code, stderr.trim_end()));
    }
    match &expected_err {
        Some(prefix) => {
            let prefix = prefix.trim_end();
            if !stderr.starts_with(prefix) {
                problems.push(format!("stderr should start with {:?}, got {:?}", prefix, stderr.trim_end()));
            }
        }
        None => {
            if !stderr.is_empty() {
                problems.push(format!("unexpected stderr: {:?}", stderr.trim_end()));
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

#[test]
fn programs() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/cases");
    let mut cases: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("tests/cases")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map_or(false, |x| x == "kpls"))
        .collect();
    cases.sort();
    assert!(cases.len() >= 25, "expected at least 25 cases, found {}", cases.len());
    let mut failures = Vec::new();
    for c in &cases {
        if let Err(msg) = run_case(c) {
            failures.push(format!("== {} ==\n{}", c.file_name().unwrap().to_string_lossy(), msg));
        }
    }
    if !failures.is_empty() {
        panic!("{} of {} cases failed:\n{}", failures.len(), cases.len(), failures.join("\n\n"));
    }
    eprintln!("{} cases passed", cases.len());
}
