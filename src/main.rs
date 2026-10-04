//! Memory Model Lab (`mml`)
//!
//! An experimental workbench for the systems-language project: it runs several
//! competing compile-time memory-safety models over the *same* tiny language
//! and reports how they diverge in what they prove and what they cost the
//! programmer. See README.md for the research framing.
//!
//! Several AST fields/methods are carried for completeness and future passes
//! even when a given checker does not read them yet; we allow dead code so the
//! data model stays honest rather than being trimmed to today's checkers.
#![allow(dead_code)]

mod ast;
mod check;
mod harness;
mod lexer;
mod metrics;
mod models;
mod parser;

use harness::{parse_expectations, Class, Verdict};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = match args.split_first() {
        Some((c, r)) => (c.as_str(), r),
        None => {
            usage();
            return ExitCode::FAILURE;
        }
    };

    match cmd {
        "check" => cmd_check(rest),
        "test" => cmd_test(rest),
        "ast" => cmd_ast(rest),
        "metrics" => cmd_metrics(rest),
        "effects" => cmd_effects(rest),
        "models" => {
            cmd_models();
            ExitCode::SUCCESS
        }
        "-h" | "--help" | "help" => {
            usage();
            ExitCode::SUCCESS
        }
        other => {
            eprintln!("unknown command: {other}\n");
            usage();
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    eprintln!(
        "mml — Memory Model Lab\n\n\
         USAGE:\n  \
         mml <command> [args]\n\n\
         COMMANDS:\n  \
         check <file.mml>       Run every model on a file and print verdicts + diagnostics\n  \
         test  <dir|files...>   Run the corpus and compare verdicts to // @expect directives\n  \
         ast   <file.mml>       Parse a file and dump its AST\n  \
         metrics <file.mml>     Print source/annotation metrics and per-model burden\n  \
         effects <file.mml>     Print per-function memory-effect summaries (the affine model's\n  \
         \x20                      interprocedural view: kills/creates/consumes/returns-alias)\n  \
         models                 List the registered memory models\n"
    );
}

fn read(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))
}

fn parse_source(source: &str) -> Result<ast::Program, String> {
    let toks = lexer::lex(source)?;
    parser::parse(toks)
}

fn cmd_models() {
    println!("Registered memory models:\n");
    for m in models::all() {
        println!("  {:<12} {}", m.name(), m.tagline());
    }
}

fn cmd_check(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else {
        eprintln!("check: expected a file path");
        return ExitCode::FAILURE;
    };
    let source = match read(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let program = match parse_source(&source) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("parse error: {e}");
            return ExitCode::FAILURE;
        }
    };

    println!("== {path} ==\n");
    for model in models::all() {
        let result = model.run(&program);
        let verdict = Verdict::of(&result);
        println!("[{}] {}", model.name(), verdict.as_str().to_uppercase());
        for d in &result.diagnostics {
            println!("    {} {}: {}", d.span, d.kind, d.message);
        }
    }
    ExitCode::SUCCESS
}

fn cmd_ast(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else {
        eprintln!("ast: expected a file path");
        return ExitCode::FAILURE;
    };
    let source = match read(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    match parse_source(&source) {
        Ok(p) => {
            println!("{p:#?}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("parse error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn cmd_metrics(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else {
        eprintln!("metrics: expected a file path");
        return ExitCode::FAILURE;
    };
    let source = match read(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let program = match parse_source(&source) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("parse error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let m = metrics::measure(&program, &source);
    println!("== metrics: {path} ==");
    println!("  source lines (code):  {}", m.source_lines);
    println!("  AST nodes:            {}", m.ast_nodes);
    println!("  annotations:");
    println!("    modes (ref/mut):    {}", m.annotations.modes);
    println!("    references (&):     {}", m.annotations.refs);
    println!("    regions:            {}", m.annotations.regions);
    println!("    frees:              {}", m.annotations.frees);
    println!("  total tokens:         {}", m.total_tokens);
    println!("  per-model burden (annotations / annotation-ratio):");
    for model in models::all() {
        println!(
            "    {:<18} {:>3}   {:>5.1}%",
            model.name(),
            m.annotations.for_model(model.name()),
            m.burden(model.name()) * 100.0
        );
    }
    ExitCode::SUCCESS
}

/// `effects <file.mml>` — print each function's memory-effect summary as the
/// affine checker computed it. Observability for the interprocedural layer:
/// the programmer never writes these; the compiler derives them.
fn cmd_effects(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else {
        eprintln!("effects: expected a file path");
        return ExitCode::FAILURE;
    };
    let source = match read(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let program = match parse_source(&source) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("parse error: {e}");
            return ExitCode::FAILURE;
        }
    };
    let (_diags, summaries, stats) = models::affine::Affine::analyze(&program);
    println!("== memory effects: {path} ==\n");
    for f in program.functions() {
        let Some(eff) = summaries.get(&f.name) else { continue };
        let params: Vec<&str> = f.params.iter().map(|p| p.name.as_str()).collect();
        let pname = |i: usize| params.get(i).copied().unwrap_or("?");
        println!("fn {}({}):   [summary facts: {}]", f.name, params.join(", "), eff.fact_count());
        for (pi, path) in &eff.kills {
            if path == "*" {
                println!("    kills:   {} (rebound — all edges)", pname(*pi));
            } else {
                println!("    kills:   {}.{path}", pname(*pi));
            }
        }
        for (ph, fld, pt) in &eff.creates {
            match fld {
                Some(p) => println!("    creates: {}.{p} -> {}", pname(*ph), pname(*pt)),
                None => println!("    creates: {} -> {}", pname(*ph), pname(*pt)),
            }
        }
        for &pi in &eff.consumes {
            println!("    consumes: {}", pname(pi));
        }
        for &pi in &eff.maybe {
            println!("    maybe-consumes: {}", pname(pi));
        }
        for &pi in &eff.ret_alias {
            println!("    returns-alias: return -> {}", pname(pi));
        }
        if eff.fact_count() == 0 {
            println!("    (no memory effects)");
        }
        println!();
    }
    println!("== fixpoint ==");
    println!(
        "SCCs: {}  (recursive: {})   largest: {} fns   max passes: {}",
        stats.scc_count, stats.recursive_sccs, stats.largest_scc, stats.max_iterations
    );
    if stats.recursive_sccs > 0 {
        println!("facts added per pass: {:?}", stats.facts_per_pass);
        println!("seeded cyclic returns: {}", stats.seeded_returns);
    }
    println!(
        "total summary facts: {}   largest summary: {}",
        stats.total_facts, stats.max_facts
    );
    ExitCode::SUCCESS
}

/// Collect `.mml` files from the given paths (files or directories).
fn collect_files(args: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let roots: Vec<&str> = if args.is_empty() {
        vec!["corpus"]
    } else {
        args.iter().map(|s| s.as_str()).collect()
    };
    for root in roots {
        let p = Path::new(root);
        if p.is_dir() {
            if let Ok(entries) = std::fs::read_dir(p) {
                let mut files: Vec<PathBuf> = entries
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().map(|e| e == "mml").unwrap_or(false))
                    .collect();
                files.sort();
                out.extend(files);
            }
        } else if p.exists() {
            out.push(p.to_path_buf());
        }
    }
    out
}

/// Per-model aggregate scored against the safety oracle.
#[derive(Default, Clone)]
struct Soundness {
    true_accept: u32,
    caught: u32,
    unsound: u32,
    over_restrictive: u32,
}

fn short_name(model: &str) -> &str {
    match model {
        "ownership" => "own",
        "region" => "reg",
        "capability" => "cap",
        "affine" => "aff",
        "region+capability" => "r+c",
        "region+ownership" => "r+o",
        "region+cap+affine" => "r+c+a",
        other => other,
    }
}

fn cmd_test(args: &[String]) -> ExitCode {
    let files = collect_files(args);
    if files.is_empty() {
        eprintln!("test: no .mml files found");
        return ExitCode::FAILURE;
    }

    let model_list = models::all();
    let names: Vec<&'static str> = model_list.iter().map(|m| m.name()).collect();

    let mut regr_total = 0u32;
    let mut regr_passed = 0u32;
    let mut parse_errors = 0u32;

    // aggregates
    let mut sound: BTreeMap<&str, Soundness> = names.iter().map(|n| (*n, Soundness::default())).collect();
    // class -> model -> (caught, total) for unsafe scenarios
    let mut matrix: BTreeMap<String, BTreeMap<&str, (u32, u32)>> = BTreeMap::new();
    // burden accumulation
    let mut ann_sum: BTreeMap<&str, u32> = names.iter().map(|n| (*n, 0)).collect();
    let mut tok_sum = 0u32;

    for path in &files {
        let source = match read(&path.to_string_lossy()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{e}");
                parse_errors += 1;
                continue;
            }
        };
        let program = match parse_source(&source) {
            Ok(p) => p,
            Err(e) => {
                println!("FAIL(parse) {}: {e}", path.display());
                parse_errors += 1;
                continue;
            }
        };
        let exp = parse_expectations(&source);
        let name = path.file_name().unwrap().to_string_lossy();
        let desc = exp.desc.clone().unwrap_or_default();
        let oracle = exp.safety;
        let class = exp.class.clone().unwrap_or_else(|| "unclassified".into());

        let m = metrics::measure(&program, &source);
        tok_sum += m.total_tokens;

        let safety_tag = match oracle {
            Some(Verdict::Reject) => "\x1b[1munsafe\x1b[0m",
            Some(Verdict::Accept) => "safe",
            None => "safety:?",
        };
        println!("• {name}  [{safety_tag}] {}", dim(&desc));

        for model in &model_list {
            let mn = model.name();
            *ann_sum.get_mut(mn).unwrap() += m.annotations.for_model(mn);
            let result = model.run(&program);
            let got = Verdict::of(&result);

            // Score against the oracle (the real finding).
            let mut oracle_mark = "     ".to_string();
            if let Some(o) = oracle {
                let cls = Class::classify(o, got);
                let s = sound.get_mut(mn).unwrap();
                match cls {
                    Class::TrueAccept => s.true_accept += 1,
                    Class::Caught => s.caught += 1,
                    Class::Unsound => s.unsound += 1,
                    Class::OverRestrictive => s.over_restrictive += 1,
                }
                oracle_mark = match cls {
                    Class::TrueAccept => "ok   ".into(),
                    Class::Caught => "\x1b[32mCAUGHT\x1b[0m".into(),
                    Class::Unsound => "\x1b[31mMISS \x1b[0m".into(),
                    Class::OverRestrictive => "\x1b[33mOVER \x1b[0m".into(),
                };
                if o == Verdict::Reject {
                    let cell = matrix.entry(class.clone()).or_default().entry(mn).or_insert((0, 0));
                    cell.1 += 1;
                    if got == Verdict::Reject {
                        cell.0 += 1;
                    }
                }
            }

            // Regression check against the recorded @expect (a behaviour lock).
            let regr = match exp.per_model.get(mn) {
                Some((want, _)) => {
                    regr_total += 1;
                    let ok = got == *want;
                    if ok {
                        regr_passed += 1;
                        "".to_string()
                    } else {
                        format!("  \x1b[31mREGRESSION: expected {}\x1b[0m", want.as_str())
                    }
                }
                None => "".to_string(),
            };

            let detail = if got == Verdict::Reject {
                first_kind(&result)
            } else {
                String::new()
            };
            println!(
                "    [{oracle_mark}] {:<18} got {:<6} {}{}",
                mn,
                got.as_str(),
                dim(&detail),
                regr
            );
        }
    }

    // ---- Safety soundness summary ----
    println!("\n=== safety soundness vs. oracle (across {} files) ===", files.len());
    println!(
        "  {:<18} {:>6} {:>7} {:>6} {:>6}",
        "model", "caught", "MISSED", "over", "ok-acc"
    );
    for n in &names {
        let s = &sound[n];
        println!(
            "  {:<18} {:>6} {:>7} {:>6} {:>6}",
            n, s.caught, s.unsound, s.over_restrictive, s.true_accept
        );
    }
    println!("  (MISSED = unsound: an unsafe program the model accepted)");

    // ---- Property x model coverage matrix ----
    println!("\n=== bug-class coverage (caught / total unsafe cases) ===");
    print!("  {:<22}", "class");
    for n in &names {
        print!("{:>6}", short_name(n));
    }
    println!();
    for (class, per_model) in &matrix {
        print!("  {:<22}", class);
        for n in &names {
            match per_model.get(n) {
                Some((c, t)) => print!("{:>6}", format!("{c}/{t}")),
                None => print!("{:>6}", "-"),
            }
        }
        println!();
    }

    // ---- Burden: annotations (B_A), states (B_S), machinery ----
    println!("\n=== burden: annotation share, state-space, and checker machinery ===");
    println!(
        "  {:<18} {:>6} {:>7} {:>7} {:>7} {:>6} {:>5} {:>5} {:>6}",
        "model", "annots", "share", "B_S", "res-st", "temp", "mv-rl", "rules", "psens"
    );
    for model in &model_list {
        let n = model.name();
        let a = ann_sum[n];
        let ratio = if tok_sum == 0 { 0.0 } else { a as f64 / tok_sum as f64 * 100.0 };
        let p = model.profile();
        println!(
            "  {:<18} {:>6} {:>6.1}% {:>7} {:>7} {:>6} {:>5} {:>5} {:>6}",
            n,
            a,
            ratio,
            p.state_count(),
            p.resource_states.len(),
            p.temporal_states,
            p.move_rules,
            p.rules,
            if p.path_sensitive { "yes" } else { "no" }
        );
    }
    println!("  (B_S = states+resource_states; res-st = states only consumables pay;");
    println!("   rules = distinct reject-rules — the rule-growth metric);");
    println!("  compiler-internal machinery (not programmer-visible):");
    for model in &model_list {
        let p = model.profile();
        println!("    {:<18} {}", model.name(), p.machinery.join(", "));
    }

    // ---- Regression verdict (the only thing that fails the run) ----
    println!("\n{regr_passed}/{regr_total} recorded @expect behaviours held");
    if parse_errors > 0 {
        println!("{parse_errors} file(s) failed to parse/read");
    }
    if regr_passed == regr_total && parse_errors == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn first_kind(result: &check::CheckResult) -> String {
    result
        .diagnostics
        .first()
        .map(|d| d.kind.to_string())
        .unwrap_or_default()
}

fn dim(s: &str) -> String {
    if s.is_empty() {
        String::new()
    } else {
        format!("\x1b[2m{s}\x1b[0m")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `// @expect` directive in the corpus must match the model's verdict.
    /// This turns the lab into a regression suite for the memory-model behaviour.
    #[test]
    fn corpus_matches_expectations() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/corpus");
        let model_list = models::all();
        let mut checked = 0u32;
        let mut failures = Vec::new();

        let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
            .expect("corpus dir")
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map(|e| e == "mml").unwrap_or(false))
            .collect();
        files.sort();
        assert!(!files.is_empty(), "corpus is empty");

        for path in files {
            let source = std::fs::read_to_string(&path).unwrap();
            let program = parse_source(&source)
                .unwrap_or_else(|e| panic!("{}: parse error: {e}", path.display()));
            let exp = parse_expectations(&source);
            assert!(
                !exp.per_model.is_empty(),
                "{}: no @expect directives",
                path.display()
            );
            for model in &model_list {
                if let Some((want, _)) = exp.per_model.get(model.name()) {
                    checked += 1;
                    let got = Verdict::of(&model.run(&program));
                    if got != *want {
                        failures.push(format!(
                            "{} [{}]: want {}, got {}",
                            path.file_name().unwrap().to_string_lossy(),
                            model.name(),
                            want.as_str(),
                            got.as_str()
                        ));
                    }
                }
            }
        }

        assert!(failures.is_empty(), "expectation mismatches:\n{}", failures.join("\n"));
        assert!(checked > 0);
    }
}
