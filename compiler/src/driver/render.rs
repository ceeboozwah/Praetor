//! N35A — the diagnostic renderer. `Diag`s carry a headline span +
//! msg, optional secondary `labels`, and trailing `notes`; this module
//! turns them into rustc-style snippet output:
//!
//! ```text
//! error: cannot consume `m`: it is borrowed
//!   --> prog.lang:32:21
//!    |
//! 32 |     let r = m.insert(kz, i64(99));
//!    |                     ^
//! note: `m` borrowed by `get`
//! ```
//!
//! Sources are registered at load time (the loader already reads every
//! file once) so the renderer never re-reads disk — a thread-local map
//! keeps the signature of every pipeline function unchanged. Caret
//! width is INFERRED from the source line: ident/number chars extend
//! the underline, punctuation stays at one — conservative by design,
//! per the proposal.
//!
//! `render_short` preserves the pre-N35A `file:line:col: error: msg`
//! format verbatim — `--diag=short` exists for scripts and editors.

use crate::Diag;
use std::cell::RefCell;
use std::collections::HashMap;

#[derive(Default)]
struct Sources {
    /// canonical path -> full source text.
    files: HashMap<String, String>,
    /// path the root file was registered under — `Diag.file == None`
    /// resolves through this.
    root: Option<String>,
}

thread_local! {
    static SOURCES: RefCell<Sources> = RefCell::new(Sources::default());
    static SHORT: RefCell<bool> = const { RefCell::new(false) };
}

/// `--diag=short` — the CLI front-end sets this once; `render_diag(s)`
/// then emits the pre-N35A one-line format for every diagnostic,
/// including those rendered inside `pipeline::build`.
pub fn set_short(v: bool) {
    SHORT.with(|m| *m.borrow_mut() = v);
}

fn short_mode() -> bool {
    SHORT.with(|m| *m.borrow())
}

/// Register a file's source text under `path` (canonical display form).
/// Call once per loaded file — the loader and `frontend_path` do this.
pub fn register_source(path: &str, src: &str) {
    SOURCES.with(|s| {
        s.borrow_mut().files.insert(path.to_string(), src.to_string());
    });
}

/// Mark `path` as the root module — `Diag.file == None` maps to it.
pub fn register_root(path: &str) {
    SOURCES.with(|s| {
        s.borrow_mut().root = Some(path.to_string());
    });
}

fn source_line(file: &Option<String>, line: u32) -> Option<String> {
    SOURCES.with(|s| {
        let m = s.borrow();
        let key = match file {
            Some(f) => f.clone(),
            None => m.root.clone()?,
        };
        let src = m.files.get(&key)?;
        src.lines().nth((line as usize).saturating_sub(1)).map(str::to_string)
    })
}

/// The display path for a diag/label: its file, or the registered root.
fn file_of(file: &Option<String>) -> Option<String> {
    SOURCES.with(|s| {
        let m = s.borrow();
        file.clone().or_else(|| m.root.clone())
    })
}

/// Expand `\t` to 4 spaces — keeping the caret column honest when the
/// source uses tabs (col counts chars; a tab is one char but four
/// display cells here).
fn untab(line: &str) -> String {
    line.replace('\t', "    ")
}

/// How many display cells `s` occupies up to char-index `n` (tabs = 4).
fn col_cells(s: &str, n: usize) -> usize {
    s.chars().take(n).map(|c| if c == '\t' { 4 } else { 1 }).sum()
}

/// Token width for the caret: ident/number-ish chars extend the
/// underline from `col` (1-based char col). Punctuation stays at 1 —
/// a one-char caret is better than highlighting the wrong range.
fn caret_width(line: &str, col: u32) -> usize {
    let start = (col as usize).saturating_sub(1);
    line.chars()
        .skip(start)
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
        .count()
        .max(1)
}

fn gutter(pad: &str, line_no: u32, text: &str, col: u32, width: usize, note: Option<&str>) -> String {
    let src = untab(text);
    let cells = col_cells(text, (col as usize).saturating_sub(1));
    let space = " ".repeat(cells);
    let carets = "^".repeat(width);
    match note {
        Some(m) => format!("{line_no:>w$} | {src}\n{pad} | {space}{carets} {m}", w = pad.len()),
        None => format!("{line_no:>w$} | {src}\n{pad} | {space}{carets}", w = pad.len()),
    }
}

/// The headline contract — `file:line:col: error: msg`, verbatim.
/// This is what `@compile-error` substring-matching and `--diag=short`
/// rely on; it must never change shape.
fn short_line(d: &Diag) -> String {
    match &d.file {
        Some(f) => format!("{f}:{}:{}: error: {}", d.span.line, d.span.col, d.msg),
        None => format!("{}:{}: error: {}", d.span.line, d.span.col, d.msg),
    }
}

/// `--diag=short` / fallback when no source is registered: the
/// pre-N35A one-line format, unchanged.
pub fn render_short(d: &Diag) -> String {
    let mut out = short_line(d);
    for l in &d.labels {
        let f = l.file.clone().or_else(|| d.file.clone());
        match f {
            Some(f) => out.push_str(&format!(
                "\n{f}:{}:{}: note: {}",
                l.span.line, l.span.col, l.msg
            )),
            None => out.push_str(&format!(
                "\n{}:{}: note: {}",
                l.span.line, l.span.col, l.msg
            )),
        }
    }
    for n in &d.notes {
        out.push_str(&format!("\nnote: {n}"));
    }
    out
}

/// Rich render — snippet + caret + secondary labels + notes.
/// Labels in foreign files get their own `-->` block; same-file
/// labels interleave under one gutter sorted by line.
/// Falls back to the short line when no source text is registered
/// for the diag's file (e.g. a `compile(src)` caller that never
/// registered a path — the headline still lands).
pub fn render(d: &Diag) -> String {
    let file = file_of(&d.file);
    let src_line = source_line(&d.file, d.span.line);
    let (Some(file), Some(src)) = (file, src_line) else {
        return render_short(d);
    };
    // (line, src, col, width, msg-or-none) for the headline file first,
    // then per foreign-file groups under their own `-->` headers.
    struct Entry { file: String, line: u32, src: String, col: u32, w: usize, msg: Option<String> }
    let mut groups: Vec<Vec<Entry>> = Vec::new();
    groups.push(vec![Entry {
        file: file.clone(), line: d.span.line, src: src.clone(),
        col: d.span.col, w: caret_width(&src, d.span.col), msg: None,
    }]);
    for l in &d.labels {
        let lf = l.file.clone().or_else(|| d.file.clone());
        let lf = file_of(&lf).unwrap_or_else(|| file.clone());
        let Some(lsrc) = source_line(&l.file.clone().or_else(|| d.file.clone()), l.span.line) else { continue };
        let e = Entry { file: lf.clone(), line: l.span.line, src: lsrc.clone(),
            col: l.span.col, w: caret_width(&lsrc, l.span.col), msg: Some(l.msg.clone()) };
        match groups.iter_mut().find(|g| g[0].file == lf) {
            Some(g) => g.push(e),
            None => groups.push(vec![e]),
        }
    }
    let max_no = groups.iter().flat_map(|g| g.iter().map(|e| e.line)).max().unwrap_or(1);
    let pad = " ".repeat(format!("{max_no}").len());
    let mut out = format!("error: {}\n", d.msg);
    for (gi, g) in groups.iter().enumerate() {
        let mut g2 = g.iter().collect::<Vec<_>>();
        g2.sort_by_key(|e| e.line);
        let e0 = &g[0];
        out.push_str(&format!("{pad}--> {}:{}:{}\n{pad} |\n", e0.file, e0.line, e0.col));
        let mut prev: Option<u32> = None;
        for e in g2 {
            if let Some(p) = prev {
                if e.line > p + 1 { out.push_str(&format!("{pad} ...\n")); }
            }
            out.push_str(&format!(
                "{}\n",
                gutter(&pad, e.line, &e.src, e.col, e.w, e.msg.as_deref())
            ));
            prev = Some(e.line);
        }
        if gi + 1 < groups.len() { out.push('\n'); }
    }
    for n in &d.notes {
        out.push_str(&format!("\n{pad} = note: {n}"));
    }
    out
}

/// Mode-aware render — honors `set_short` (the `--diag=short` path).
pub fn render_diag(d: &Diag) -> String {
    if short_mode() { render_short(d) } else { render(d) }
}

pub fn render_all(ds: &[Diag]) -> String {
    ds.iter().map(render_diag).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Diag, Label, Span};

    fn sp(line: u32, col: u32) -> Span {
        Span { line, col }
    }

    fn reset() {
        SOURCES.with(|s| {
            *s.borrow_mut() = Sources::default();
        });
        set_short(false);
    }

    const SRC: &str = "fn main() -> i32 {\n    let heap = system_allocator();\n    let m = alloc<i32>(heap);\n    let q = m;\n    return *m;\n}\n";

    #[test]
    fn short_is_the_old_format_verbatim() {
        reset();
        register_source("t.lang", SRC);
        register_root("t.lang");
        set_short(true);
        let d = Diag::new(sp(5, 13), "use after consume `m`");
        assert_eq!(render_diag(&d), "5:13: error: use after consume `m`");
        let mut dm = d.clone();
        dm.file = Some("lib/m.lang".into());
        assert_eq!(
            render_diag(&dm),
            "lib/m.lang:5:13: error: use after consume `m`"
        );
    }

    #[test]
    fn rich_keeps_the_headline_and_adds_snippet() {
        reset();
        register_source("t.lang", SRC);
        register_root("t.lang");
        let d = Diag::new(sp(5, 13), "use after consume `m`");
        let out = render(&d);
        assert!(out.starts_with("error: use after consume `m`\n"));
        assert!(out.contains("--> t.lang:5:13"));
        assert!(out.contains("5 |     return *m;"));
        assert!(out.contains("             ^")); // caret under `m`
    }

    #[test]
    fn no_source_falls_back_to_short() {
        reset();
        let d = Diag::new(sp(5, 13), "use after consume `m`");
        assert_eq!(render(&d), "5:13: error: use after consume `m`");
    }

    #[test]
    fn labels_and_notes_render() {
        reset();
        register_source("t.lang", SRC);
        register_root("t.lang");
        let d = Diag::new(sp(5, 13), "cannot consume `m`: it is borrowed")
            .label(sp(4, 13), "borrow created here")
            .note("the borrow is live until `q`'s last use");
        let out = render(&d);
        assert!(out.contains("4 |     let q = m;"));
        assert!(out.contains("borrow created here"));
        assert!(out.contains("= note: the borrow is live until `q`'s last use"));
    }

    #[test]
    fn tabs_expand_and_utf8_aligns() {
        reset();
        register_source("t.lang", "fn m() -> i32 {\n\tlet x = h\u{e9}llo + true;\n\treturn 0;\n}\n");
        register_root("t.lang");
        let d = Diag::new(sp(2, 17), "arithmetic on non-integers: i32 and bool");
        let out = render(&d);
        assert!(out.contains("2 |     let x = h\u{e9}llo + true;"));
        // caret under `+`: tab->4 cells + 15 chars before it = 19 cells
        assert!(out.contains("                   ^"));
    }

    #[test]
    fn caret_width_table() {
        // conservative: idents and qualified paths extend; punctuation stays 1
        assert_eq!(caret_width("    foo.bar(1)", 5), 7);   // foo.bar
        assert_eq!(caret_width("    x[i]", 5), 1);          // x[i] -> `x` only? col on x
        assert_eq!(caret_width("    x[i]", 5), 1);
        assert_eq!(caret_width("    Vec<i32>", 5), 3);       // Vec
        assert_eq!(caret_width("    &mut *p", 5), 1);        // &
        assert_eq!(caret_width("    &mut *p", 6), 3);        // mut
        assert_eq!(caret_width("    0xff", 5), 4);           // 0xff
        assert_eq!(caret_width("    a::b", 5), 1);           // no `::` in continue-set
    }
}
