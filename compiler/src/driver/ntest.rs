//! The native test runner — the second corpus. Each `native-tests/*.lang`
//! carries directives in `//` comments:
//!
//!   // @exit <n>            compile must succeed; run exit code == n
//!   // @compile-error <s>   compile must fail; message contains s
//!   // @args a "b c" ""     run the binary with these argv (N26A)
//!
//! This is the codegen counterpart of the lab's `@expect` oracle: the
//! semantic corpus proves the checker; this one proves the silicon.

use super::pipeline;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Case {
    path: PathBuf,
    exit: Option<i32>,
    compile_err: Option<String>,
    args: Vec<String>,
}

/// `@args` tokens — whitespace-separated, `"..."` groups spaces and
/// may produce an empty argument (`""`).
fn parse_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut seen = false;
    for c in s.chars() {
        if c == '"' {
            in_q = !in_q;
            seen = true;
        } else if c.is_whitespace() && !in_q {
            if seen || !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
                seen = false;
            }
        } else {
            cur.push(c);
            seen = true;
        }
    }
    if seen || !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn parse_case(path: &Path) -> Result<Case, String> {
    let src = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut c = Case {
        path: path.to_path_buf(),
        exit: None,
        compile_err: None,
        args: Vec::new(),
    };
    for line in src.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("// @exit") {
            c.exit = Some(
                rest.trim()
                    .parse()
                    .map_err(|_| format!("{}: bad @exit value", path.display()))?,
            );
        } else if let Some(rest) = line.strip_prefix("// @compile-error") {
            c.compile_err = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("// @args") {
            c.args = parse_args(rest.trim());
        }
    }
    if c.exit.is_none() && c.compile_err.is_none() {
        return Err(format!("{}: no @exit/@compile-error directive", path.display()));
    }
    Ok(c)
}

/// Run every `.lang` in `dir`. Returns (passed, failed, report).
pub fn run_dir(dir: &Path) -> (usize, usize, String) {
    let mut report = String::new();
    let mut cases: Vec<Case> = Vec::new();
    let mut paths: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| {
                let p = e.unwrap().path();
                (p.extension().map(|x| x == "lang").unwrap_or(false)).then_some(p)
            })
            .collect(),
        Err(e) => return (0, 1, format!("cannot read {}: {e}", dir.display())),
    };
    paths.sort();
    for p in &paths {
        match parse_case(p) {
            Ok(c) => cases.push(c),
            Err(e) => {
                report.push_str(&format!("  SKIP  {e}\n"));
            }
        }
    }
    let workdir = std::env::temp_dir().join("ourc-native-tests");
    let (mut pass, mut fail) = (0, 0);
    for c in &cases {
        let name = c.path.file_name().unwrap().to_string_lossy();
        match (&c.compile_err, pipeline::build(&c.path, &workdir)) {
            (Some(want), Err(msg)) => {
                if msg.contains(want.as_str()) {
                    pass += 1;
                    report.push_str(&format!("  ok    {name} (compile error matched)\n"));
                } else {
                    fail += 1;
                    report.push_str(&format!(
                        "  FAIL  {name}: wanted error containing `{want}`, got:\n{msg}"
                    ));
                }
            }
            (Some(want), Ok(_)) => {
                fail += 1;
                report.push_str(&format!(
                    "  FAIL  {name}: compiled but wanted error `{want}`\n"
                ));
            }
            (None, Err(msg)) => {
                fail += 1;
                report.push_str(&format!("  FAIL  {name}: compile failed:\n{msg}"));
            }
            (None, Ok(exe)) => {
                let out = Command::new(&exe).args(&c.args).output();
                match out {
                    Ok(o) => {
                        let code = o.status.code().unwrap_or(-1);
                        if Some(code) == c.exit {
                            pass += 1;
                            report.push_str(&format!("  ok    {name} -> exit {code}\n"));
                        } else {
                            fail += 1;
                            report.push_str(&format!(
                                "  FAIL  {name}: exit {code}, wanted {}\n",
                                c.exit.unwrap()
                            ));
                        }
                    }
                    Err(e) => {
                        fail += 1;
                        report.push_str(&format!("  FAIL  {name}: run failed: {e}\n"));
                    }
                }
            }
        }
    }
    report.push_str(&format!("\n{pass} passed, {fail} failed\n"));
    (pass, fail, report)
}
