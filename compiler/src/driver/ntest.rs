//! The native test runner — the second corpus. Each `native-tests/*.lang`
//! carries directives in `//` comments:
//!
//!   // @exit <n>            compile must succeed; run exit code == n
//!   // @compile-error <s>   compile must fail; message contains s
//!   // @args a "b c" ""     run the binary with these argv (N26A)
//!   // @ir-has <s>          emitted .ll must contain s (repeatable)
//!   // @ir-lacks <s>        emitted .ll must not contain s (repeatable)
//!   // @stdout <file>       run stdout must equal <file> byte-exact
//!                         (resolved next to the fixture; .gitattributes
//!                         pins LF so the oracle is platform-stable)
//!   // @skip <platform>     SKIP on the named host — `windows` or
//!                         `posix`; for provably platform-assuming
//!                         fixtures (N79C: /etc/hosts, HOME, /bin/sh)
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
    ir_has: Vec<String>,
    ir_lacks: Vec<String>,
    stdout: Option<String>,
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
        ir_has: Vec::new(),
        ir_lacks: Vec::new(),
        stdout: None,
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
        } else if let Some(rest) = line.strip_prefix("// @ir-has") {
            c.ir_has.push(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("// @ir-lacks") {
            c.ir_lacks.push(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("// @stdout") {
            c.stdout = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("// @skip") {
            let plat = rest.trim().split_whitespace().next().unwrap_or("");
            let hit = match plat {
                "windows" => cfg!(windows),
                "posix" => cfg!(unix),
                _ => false,
            };
            if hit {
                return Err(format!("{}: @skip{}", path.display(), rest));
            }
        }
    }
    if c.exit.is_none()
        && c.compile_err.is_none()
        && c.ir_has.is_empty()
        && c.ir_lacks.is_empty()
        && c.stdout.is_none()
    {
        return Err(format!(
            "{}: no @exit/@compile-error/@ir-has/@ir-lacks/@stdout directive",
            path.display()
        ));
    }
    Ok(c)
}

/// `@stdout` mismatch detail — first differing byte offset and an
/// escaped window around it from both expected and actual output.
fn stdout_diff(want: &[u8], got: &[u8]) -> String {
    let off = want
        .iter()
        .zip(got.iter())
        .position(|(a, b)| a != b)
        .unwrap_or(want.len().min(got.len()));
    let esc = |b: &[u8]| -> String {
        b.iter()
            .map(|&c| match c {
                b'\n' => "\\n".to_string(),
                b'\r' => "\\r".to_string(),
                b'\t' => "\\t".to_string(),
                0x20..=0x7e => (c as char).to_string(),
                _ => format!("\\x{c:02x}"),
            })
            .collect()
    };
    let ctx = |b: &[u8]| esc(&b[off.saturating_sub(16)..(off + 16).min(b.len())]);
    format!(
        "         first diff at byte {off} (want {} bytes, got {})\n         want: ...{}\n         got:  ...{}\n",
        want.len(),
        got.len(),
        ctx(want),
        ctx(got)
    )
}

/// Run every `.lang` in `dir`. Returns (passed, failed, report).
/// `roots` carries declared package roots (N38) so test files can
/// `import` the project's dependencies.
pub fn run_dir(
    dir: &Path,
    roots: &std::collections::HashMap<String, PathBuf>,
) -> (usize, usize, String) {
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
        match (
            &c.compile_err,
            pipeline::build_with(&c.path, &workdir, roots, false, false),
        ) {
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
                // IR-shape assertions (N51): grep the emitted `.ll`
                // that `build_with` leaves next to the binary.
                let ll = exe.with_extension("ll");
                let text = std::fs::read_to_string(&ll).unwrap_or_default();
                let mut bad = false;
                for s in &c.ir_has {
                    if !text.contains(s.as_str()) {
                        fail += 1;
                        bad = true;
                        report.push_str(&format!(
                            "  FAIL  {name}: .ll lacks `{s}`\n"
                        ));
                    }
                }
                for s in &c.ir_lacks {
                    if text.contains(s.as_str()) {
                        fail += 1;
                        bad = true;
                        report.push_str(&format!(
                            "  FAIL  {name}: .ll contains forbidden `{s}`\n"
                        ));
                    }
                }
                if bad {
                    continue;
                }
                if c.exit.is_none() && c.stdout.is_none() {
                    pass += 1;
                    report.push_str(&format!("  ok    {name} (ir shape)\n"));
                    continue;
                }
                let out = Command::new(&exe).args(&c.args).output();
                match out {
                    Ok(o) => {
                        let code = o.status.code().unwrap_or(-1);
                        // N58C — `@exit -1` means "terminated
                        // abnormally": POSIX signals report no exit
                        // code (unwrap_or gives -1), while Windows
                        // abort() is code 3 and SEH faults land as
                        // high-bit exception codes (negative as i32).
                        let mut bad = false;
                        if let Some(exit) = c.exit {
                            let matched = if exit == -1 && cfg!(windows) {
                                code == 3 || code < 0
                            } else {
                                code == exit
                            };
                            if !matched {
                                bad = true;
                                report.push_str(&format!(
                                    "  FAIL  {name}: exit {code}, wanted {exit}\n"
                                ));
                            }
                        }
                        if let Some(reffile) = &c.stdout {
                            let refp = c
                                .path
                                .parent()
                                .unwrap_or(Path::new("."))
                                .join(reffile);
                            match std::fs::read(&refp) {
                                Err(e) => {
                                    bad = true;
                                    report.push_str(&format!(
                                        "  FAIL  {name}: @stdout oracle {}: {e}\n",
                                        refp.display()
                                    ));
                                }
                                Ok(want) => {
                                    if want != o.stdout {
                                        bad = true;
                                        report.push_str(&format!(
                                            "  FAIL  {name}: stdout mismatch vs {reffile}\n{}",
                                            stdout_diff(&want, &o.stdout)
                                        ));
                                    }
                                }
                            }
                        }
                        if bad {
                            fail += 1;
                        } else {
                            pass += 1;
                            report.push_str(&format!("  ok    {name} -> exit {code}\n"));
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
