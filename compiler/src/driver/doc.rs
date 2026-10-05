//! `our doc <pkg>.<mod>` — the generated API reference (N45). Reads a
//! module's own source and renders its public surface: `pub` fns with
//! resolved signatures, `pub` structs/enums with fields and variants,
//! and `///` doc comments lifted from the token stream's leading
//! trivia (N39A). It deliberately never touches the semantic pass —
//! a doc comment can never change what compiles.

use crate::frontend::ast::{Auth, FnDef};
use crate::frontend::lexer::{self, Tok, Token, Trivia};
use crate::frontend::parser;
use crate::Span;
use std::collections::HashMap;
use std::path::PathBuf;

/// `///` lines leading a token, marker stripped. `////` stays an
/// ordinary comment — the fourth slash is content, not a sigil.
fn doc_lines(t: &Token) -> Vec<String> {
    t.leading
        .iter()
        .filter_map(|tr| match tr {
            Trivia::Line { text, .. }
                if text.starts_with("///") && !text.starts_with("////") =>
            {
                Some(text[3..].trim().to_string())
            }
            _ => None,
        })
        .collect()
}

/// Resolve `std.io` → stdlib, `pkg.mod` → the package's `src/` —
/// the same first-segment rule the module loader uses.
fn resolve(path: &[String], roots: &HashMap<String, PathBuf>) -> Result<PathBuf, String> {
    let Some(first) = path.first() else {
        return Err("empty module path".into());
    };
    let rest: PathBuf = path[1..].iter().collect();
    if rest.as_os_str().is_empty() {
        return Err(format!(
            "`{first}` names a package root — doc a module: `{first}.<mod>`"
        ));
    }
    let base = root_of(first, roots)?.join(&rest);
    let file = base.with_extension("lang");
    if file.is_file() {
        Ok(file)
    } else {
        Err(format!(
            "no module file `{}` — `our doc {first}` lists {first}'s modules",
            file.display()
        ))
    }
}

/// `read`/`write`/`own` ahead of a param name — `None` renders bare,
/// same convention as the checker's hover.
fn auth_str(a: Option<Auth>) -> &'static str {
    match a {
        Some(Auth::Read) => "read ",
        Some(Auth::Write) => "write ",
        Some(Auth::Own) => "own ",
        _ => "",
    }
}

fn tparams(ps: &[String]) -> String {
    if ps.is_empty() {
        String::new()
    } else {
        format!("<{}>", ps.join(","))
    }
}

fn fn_sig(f: &FnDef) -> String {
    let params: Vec<String> = f
        .params
        .iter()
        .map(|(n, a, t)| format!("{}{n}: {t}", auth_str(*a)))
        .collect();
    format!(
        "{} {}{}({}) -> {}",
        if f.is_extern { "extern fn" } else { "fn" },
        f.name,
        tparams(&f.tparams),
        params.join(", "),
        f.ret
    )
}

/// Find the `///` docs for a decl. AST spans sit on the `fn`/`struct`/
/// `enum` keyword, but `///` comments lead the whole decl — so for
/// `pub`/`extern`-prefixed decls they live on the prefix token.
fn decl_docs(at: &HashMap<Span, usize>, toks: &[Token], span: Span) -> Vec<String> {
    let Some(&i) = at.get(&span) else { return Vec::new() };
    if i > 0 && matches!(&toks[i - 1].tok, Tok::Ident(n) if n == "pub" || n == "extern") {
        let d = doc_lines(&toks[i - 1]);
        if !d.is_empty() {
            return d;
        }
    }
    doc_lines(&toks[i])
}

/// Render one module's public API as deterministic text: `std.` /
/// package path header, module doc, then structs, enums and fns in
/// source order within each group. Private decls — including externs,
/// which are module-local C names — never appear.
/// The package root a path's first segment names — `std` or a
/// project root. Shared by module resolution and bare-package
/// listing.
fn root_of(first: &str, roots: &HashMap<String, PathBuf>) -> Result<PathBuf, String> {
    if first == "std" {
        Ok(crate::driver::pipeline::stdlib_dir())
    } else if let Some(root) = roots.get(first) {
        Ok(root.clone())
    } else {
        Err(format!(
            "`{first}` is not a known package — `std` or a [dependencies]/own-package name"
        ))
    }
}

/// `our doc std` / `our doc <pkg>` — list the package's modules so
/// the API surface is discoverable without reading its directory.
fn list_modules(first: &str, roots: &HashMap<String, PathBuf>) -> Result<String, String> {
    let dir = root_of(first, roots)?;
    let mut mods: Vec<String> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| {
                    let p = e.path();
                    p.extension()
                        .map(|x| x == "lang")
                        .unwrap_or(false)
                        .then(|| p.file_stem().unwrap_or_default().to_string_lossy().to_string())
                })
                .collect()
        })
        .unwrap_or_default();
    mods.sort();
    let mut out = format!("{first}\n");
    for m in mods {
        out.push_str(&format!("\n{first}.{m}\n"));
    }
    Ok(out)
}

/// Render one module's public API as deterministic text: `std.` /
/// package path header, module doc, then structs, enums and fns in
/// source order within each group. Private decls — including externs,
/// which are module-local C names — never appear.
pub fn render(path: &[String], roots: &HashMap<String, PathBuf>) -> Result<String, String> {
    if path.len() == 1 {
        return list_modules(&path[0], roots);
    }
    let file = resolve(path, roots)?;
    let src = std::fs::read_to_string(&file)
        .map_err(|e| format!("{}: {e}", file.display()))?;
    let toks = lexer::lex(&src).map_err(|ds| {
        ds.first()
            .map(|d| format!("{}:{}:{}: {}", file.display(), d.span.line, d.span.col, d.msg))
            .unwrap_or_else(|| format!("{}: does not lex", file.display()))
    })?;
    let prog = parser::parse(&src).map_err(|ds| {
        ds.first()
            .map(|d| format!("{}:{}:{}: {}", file.display(), d.span.line, d.span.col, d.msg))
            .unwrap_or_else(|| format!("{}: does not parse", file.display()))
    })?;
    let at: HashMap<Span, usize> = toks
        .iter()
        .enumerate()
        .map(|(i, t)| (t.span, i))
        .collect();
    // The file-leading `///` run is the module's own doc — except
    // when the file opens with `pub`, where it plainly documents
    // that decl. (A leading block ahead of `module`/`import`/
    // `extern`/`fn` has no public decl to attach to anyway.)
    let module_doc = toks
        .first()
        .filter(|t| !matches!(&t.tok, Tok::Ident(n) if n == "pub"))
        .map(doc_lines)
        .unwrap_or_default();

    let mut out = format!("{}\n", path.join("."));
    let mut section = false;
    let mut emit = |docs: &[String], sig: String| {
        out.push('\n');
        for d in docs {
            out.push_str(d);
            out.push('\n');
        }
        out.push_str(&sig);
        out.push('\n');
        section = true;
    };

    for s in &prog.structs {
        if !s.is_pub {
            continue;
        }
        let mut sig = format!("struct {}{} {{\n", s.name, tparams(&s.tparams));
        for (n, t) in &s.fields {
            sig.push_str(&format!("    {n}: {t},\n"));
        }
        sig.push('}');
        emit(&decl_docs(&at, &toks, s.span), sig);
    }
    for e in &prog.enums {
        if !e.is_pub {
            continue;
        }
        let mut sig = format!("enum {}{} {{\n", e.name, tparams(&e.tparams));
        for (n, p) in &e.variants {
            match p {
                Some(t) => sig.push_str(&format!("    {n}({t}),\n")),
                None => sig.push_str(&format!("    {n},\n")),
            }
        }
        sig.push('}');
        emit(&decl_docs(&at, &toks, e.span), sig);
    }
    for f in &prog.fns {
        if !f.is_pub {
            continue;
        }
        emit(&decl_docs(&at, &toks, f.span), fn_sig(f));
    }
    if !section && module_doc.is_empty() {
        out.push_str("\n(no public declarations)\n");
    }
    // Module doc rides directly under the header.
    if !module_doc.is_empty() {
        let body = out.split_off(path.join(".").len() + 1);
        out.push('\n');
        for d in &module_doc {
            out.push_str(&d);
            out.push('\n');
        }
        out.push_str(&body);
    }
    Ok(out)
}

/// `our doc <pkg>.<mod>` — stdout is the reference; resolution errors
/// go to stderr with the same package-vs-module distinction the
/// import loader draws.
pub fn run(arg: &str) -> Result<(), String> {
    let path: Vec<String> = arg.split('.').map(String::from).collect();
    let roots = std::env::current_dir()
        .ok()
        .and_then(|d| crate::driver::project::discover(&d).ok())
        .and_then(|p| {
            crate::driver::project::resolve_deps(&p.root, &p.manifest)
                .ok()
                .map(|deps| crate::driver::project::roots_with_self(&p, &deps))
        })
        .unwrap_or_default();
    print!("{}", render(&path, &roots)?);
    Ok(())
}
