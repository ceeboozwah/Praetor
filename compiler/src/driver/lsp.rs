//! N36A — a minimal LSP server over stdio JSON-RPC. Scope is
//! deliberately narrow: `initialize`, `shutdown`, `exit`,
//! `textDocument/didOpen`, `didChange` (full sync), `didClose`, and
//! `textDocument/publishDiagnostics`. No incremental parsing — a
//! change re-runs the whole frontend, which is fast enough at current
//! project sizes.
//!
//! Mapping (`Diag` → LSP `Diagnostic`):
//!   `span`/`file` → `range` (0-based, UTF-16 code units)
//!   `msg`         → `message` (notes appended as `note:` lines)
//!   `code()`      → `code` (`"E1001"`)
//!   `labels`      → `relatedInformation` — module-crossing labels get
//!                   the module's own `file://` URI
//!
//! Position conversion lives in ONE place: `utf16_col`. Compiler
//! `col` is a 1-based char index; LSP wants 0-based UTF-16 units —
//! identical for ASCII, divergent for non-BMP chars (emoji = 2 units).

use crate::driver::pipeline;
use crate::driver::render;
use crate::Diag;
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Minimal JSON value — enough for LSP. Objects keep insertion order
/// so generated messages read deterministically.
#[derive(Clone, Debug, PartialEq)]
enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn obj(pairs: Vec<(&str, Json)>) -> Json {
        Json::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }
    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
    fn s(&self) -> &str {
        match self {
            Json::Str(s) => s,
            _ => "",
        }
    }
    fn write(&self, out: &mut String) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Json::Num(n) => {
                if *n == n.trunc() && n.is_finite() {
                    out.push_str(&format!("{}", *n as i64));
                } else {
                    out.push_str(&format!("{n}"));
                }
            }
            Json::Str(s) => {
                out.push('"');
                for c in s.chars() {
                    match c {
                        '"' => out.push_str("\\\""),
                        '\\' => out.push_str("\\\\"),
                        '\n' => out.push_str("\\n"),
                        '\r' => out.push_str("\\r"),
                        '\t' => out.push_str("\\t"),
                        c if (c as u32) < 0x20 => {
                            out.push_str(&format!("\\u{:04x}", c as u32));
                        }
                        c => out.push(c),
                    }
                }
                out.push('"');
            }
            Json::Arr(xs) => {
                out.push('[');
                for (i, x) in xs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    x.write(out);
                }
                out.push(']');
            }
            Json::Obj(pairs) => {
                out.push('{');
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    Json::Str(k.clone()).write(out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

struct JsonParser<'a> {
    b: &'a [u8],
    i: usize,
}

impl<'a> JsonParser<'a> {
    fn ws(&mut self) {
        while matches!(self.b.get(self.i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.i += 1;
        }
    }
    fn value(&mut self) -> Option<Json> {
        self.ws();
        match *self.b.get(self.i)? {
            b'{' => self.object(),
            b'[' => self.array(),
            b'"' => Some(Json::Str(self.string()?)),
            b't' => self.lit("true", Json::Bool(true)),
            b'f' => self.lit("false", Json::Bool(false)),
            b'n' => self.lit("null", Json::Null),
            _ => self.number(),
        }
    }
    fn lit(&mut self, w: &str, v: Json) -> Option<Json> {
        if self.b[self.i..].starts_with(w.as_bytes()) {
            self.i += w.len();
            Some(v)
        } else {
            None
        }
    }
    fn number(&mut self) -> Option<Json> {
        let start = self.i;
        while matches!(
            self.b.get(self.i),
            Some(b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
        ) {
            self.i += 1;
        }
        std::str::from_utf8(&self.b[start..self.i])
            .ok()?
            .parse()
            .ok()
            .map(Json::Num)
    }
    fn hex4(&mut self) -> Option<u32> {
        let h = std::str::from_utf8(self.b.get(self.i..self.i + 4)?).ok()?;
        let v = u32::from_str_radix(h, 16).ok()?;
        self.i += 4;
        Some(v)
    }
    fn string(&mut self) -> Option<String> {
        if *self.b.get(self.i)? != b'"' {
            return None;
        }
        self.i += 1;
        let mut s = String::new();
        loop {
            match *self.b.get(self.i)? {
                b'"' => {
                    self.i += 1;
                    return Some(s);
                }
                b'\\' => {
                    self.i += 1;
                    match *self.b.get(self.i)? {
                        b'"' => s.push('"'),
                        b'\\' => s.push('\\'),
                        b'/' => s.push('/'),
                        b'b' => s.push('\u{8}'),
                        b'f' => s.push('\u{c}'),
                        b'n' => s.push('\n'),
                        b'r' => s.push('\r'),
                        b't' => s.push('\t'),
                        b'u' => {
                            self.i += 1;
                            let hi = self.hex4()?;
                            if (0xD800..0xDC00).contains(&hi)
                                && self.b.get(self.i..self.i + 2) == Some(b"\\u".as_slice())
                            {
                                self.i += 2;
                                let lo = self.hex4()?;
                                let cp =
                                    0x10000 + ((hi - 0xD800) << 10) + (lo.wrapping_sub(0xDC00));
                                s.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                            } else {
                                s.push(char::from_u32(hi).unwrap_or('\u{fffd}'));
                            }
                            continue;
                        }
                        _ => return None,
                    }
                    self.i += 1;
                }
                _ => {
                    let rest = std::str::from_utf8(&self.b[self.i..]).ok()?;
                    let c = rest.chars().next()?;
                    s.push(c);
                    self.i += c.len_utf8();
                }
            }
        }
    }
    fn object(&mut self) -> Option<Json> {
        self.i += 1; // '{'
        let mut pairs = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b'}') {
            self.i += 1;
            return Some(Json::Obj(pairs));
        }
        loop {
            self.ws();
            let k = self.string()?;
            self.ws();
            if *self.b.get(self.i)? != b':' {
                return None;
            }
            self.i += 1;
            let v = self.value()?;
            pairs.push((k, v));
            self.ws();
            match *self.b.get(self.i)? {
                b',' => self.i += 1,
                b'}' => {
                    self.i += 1;
                    return Some(Json::Obj(pairs));
                }
                _ => return None,
            }
        }
    }
    fn array(&mut self) -> Option<Json> {
        self.i += 1; // '['
        let mut xs = Vec::new();
        self.ws();
        if self.b.get(self.i) == Some(&b']') {
            self.i += 1;
            return Some(Json::Arr(xs));
        }
        loop {
            xs.push(self.value()?);
            self.ws();
            match *self.b.get(self.i)? {
                b',' => self.i += 1,
                b']' => {
                    self.i += 1;
                    return Some(Json::Arr(xs));
                }
                _ => return None,
            }
        }
    }
}

fn parse_json(body: &[u8]) -> Option<Json> {
    JsonParser { b: body, i: 0 }.value()
}

/// `file:///abs/path` ↔ `/abs/path` — percent-decode on the way in,
/// percent-encode on the way out. Only the characters that matter
/// (`%`, space, `#`, `?`, non-ASCII) are encoded.
fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let host_end = rest.find('/')?;
    let mut out = Vec::new();
    let bs = rest[host_end..].as_bytes();
    let mut i = 0;
    while i < bs.len() {
        if bs[i] == b'%' && i + 2 < bs.len() {
            let h = std::str::from_utf8(&bs[i + 1..i + 3]).ok()?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(bs[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok().map(PathBuf::from)
}

fn path_to_uri(p: &Path) -> String {
    let mut s = String::from("file://");
    for &b in p.to_string_lossy().as_bytes() {
        match b {
            b'%' | b' ' | b'#' | b'?' => s.push_str(&format!("%{b:02X}")),
            b if b < 0x80 => s.push(b as char),
            b => s.push_str(&format!("%{b:02X}")),
        }
    }
    s
}

/// 1-based char col → 0-based UTF-16 code units. `🚀` costs 2, `é` 1.
fn utf16_col(line: &str, col: u32) -> u32 {
    line.chars()
        .take((col as usize).saturating_sub(1))
        .map(|c| c.len_utf16() as u32)
        .sum()
}

/// One `Span`+file → an LSP `Range`. The end col extends by the same
/// conservative `caret_width` the renderer uses, converted to UTF-16.
fn lsp_range(file: &Option<String>, line: u32, col: u32) -> Json {
    let text = render::source_line(file, line).unwrap_or_default();
    let start = utf16_col(&text, col);
    let w = render::caret_width(&text, col);
    let end_utf16: u32 = text
        .chars()
        .skip((col as usize).saturating_sub(1))
        .take(w)
        .map(|c| c.len_utf16() as u32)
        .sum();
    let ln = line.saturating_sub(1);
    Json::obj(vec![
        (
            "start",
            Json::obj(vec![
                ("line", Json::Num(ln as f64)),
                ("character", Json::Num(start as f64)),
            ]),
        ),
        (
            "end",
            Json::obj(vec![
                ("line", Json::Num(ln as f64)),
                ("character", Json::Num((start + end_utf16) as f64)),
            ]),
        ),
    ])
}

/// `Diag.file` → the URI a diagnostic or related-information entry
/// publishes under. Root-file diags (`file: None`) map to the open
/// document's own URI — never a canonicalized path, since the editor
/// keys diagnostics by the URI string it opened.
fn file_uri(file: &Option<String>, root_uri: &str) -> String {
    match file {
        Some(f) => path_to_uri(Path::new(f)),
        None => root_uri.to_string(),
    }
}

fn diag_to_lsp(d: &Diag, root_uri: &str) -> Json {
    let mut msg = d.msg.clone();
    for n in &d.notes {
        msg.push_str(&format!("\nnote: {n}"));
    }
    let related: Vec<Json> = d
        .labels
        .iter()
        .map(|l| {
            Json::obj(vec![
                (
                    "location",
                    Json::obj(vec![
                        ("uri", Json::Str(file_uri(&l.file, root_uri))),
                        ("range", lsp_range(&l.file, l.span.line, l.span.col)),
                    ]),
                ),
                ("message", Json::Str(l.msg.clone())),
            ])
        })
        .collect();
    let mut fields = vec![
        ("range", lsp_range(&d.file, d.span.line, d.span.col)),
        ("severity", Json::Num(1.0)),
        ("source", Json::Str("ourc".into())),
        ("message", Json::Str(msg)),
    ];
    if let Some(c) = d.code() {
        fields.push(("code", Json::Str(c.into())));
    }
    if !related.is_empty() {
        fields.push(("relatedInformation", Json::Arr(related)));
    }
    Json::obj(fields)
}

/// Re-check a document; returns the diagnostics grouped by file URI.
/// `stale` gets the URIs that previously had diagnostics and must be
/// cleared if they're clean now.
fn run_diagnostics(path: &Path, uri: &str, text: &str) -> BTreeMap<String, Vec<Json>> {
    let mut by_uri: BTreeMap<String, Vec<Json>> = BTreeMap::new();
    if let Err(ds) = pipeline::check_source(path, text) {
        for d in &ds {
            by_uri
                .entry(file_uri(&d.file, uri))
                .or_default()
                .push(diag_to_lsp(d, uri));
        }
    }
    by_uri
}

fn publish(uri: &str, diags: Vec<Json>) -> Json {
    Json::obj(vec![
        ("jsonrpc", Json::Str("2.0".into())),
        ("method", Json::Str("textDocument/publishDiagnostics".into())),
        (
            "params",
            Json::obj(vec![
                ("uri", Json::Str(uri.to_string())),
                ("diagnostics", Json::Arr(diags)),
            ]),
        ),
    ])
}

fn frame(j: &Json) -> Vec<u8> {
    let mut body = String::new();
    j.write(&mut body);
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes()
}

/// `ourc lsp` — read Content-Length-framed JSON-RPC from stdin, write
/// responses/notifications to stdout, until `exit`.
pub fn serve() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut inp = stdin.lock();
    // open URI -> (path, buffer text)
    let mut docs: HashMap<String, (PathBuf, String)> = HashMap::new();
    // URIs that currently carry diagnostics — a clean re-check must
    // publish an explicit empty array for each of these.
    let mut dirty: std::collections::BTreeSet<String> = Default::default();

    loop {
        // frame read
        let mut header = String::new();
        let mut byte = [0u8; 1];
        loop {
            if inp.read(&mut byte).unwrap_or(0) == 0 {
                return; // EOF — client gone
            }
            header.push(byte[0] as char);
            if header.ends_with("\r\n\r\n") {
                break;
            }
        }
        let len: usize = header
            .split("\r\n")
            .find_map(|l| l.strip_prefix("Content-Length: "))
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0);
        let mut body = vec![0u8; len];
        if inp.read_exact(&mut body).is_err() {
            return;
        }
        let Some(msg) = parse_json(&body) else {
            continue;
        };

        let method = msg.get("method").map(|m| m.s()).unwrap_or("");
        let id = msg.get("id").cloned();
        let params = msg.get("params");
        let doc_uri = || {
            params
                .and_then(|p| p.get("textDocument"))
                .and_then(|t| t.get("uri"))
                .map(|u| u.s().to_string())
        };

        let mut out: Vec<u8> = Vec::new();
        let respond = |out: &mut Vec<u8>, result: Json| {
            if let Some(id) = &id {
                out.extend(frame(&Json::obj(vec![
                    ("jsonrpc", Json::Str("2.0".into())),
                    ("id", id.clone()),
                    ("result", result),
                ])));
            }
        };
        // Re-check `uri`'s buffer and publish; also clears any URIs
        // whose diagnostics all resolved.
        macro_rules! recheck {
            ($out:expr, $uri:expr) => {{
                if let Some((path, text)) = docs.get(&$uri) {
                    let by_uri = run_diagnostics(path, &$uri, text);
                    let mut to_clear: Vec<String> =
                        dirty.iter().filter(|u| !by_uri.contains_key(*u)).cloned().collect();
                    for (u, ds) in by_uri {
                        dirty.insert(u.clone());
                        $out.extend(frame(&publish(&u, ds)));
                    }
                    for u in to_clear.drain(..) {
                        dirty.remove(&u);
                        $out.extend(frame(&publish(&u, vec![])));
                    }
                }
            }};
        }

        match method {
            "initialize" => respond(&mut out, Json::obj(vec![(
                "capabilities",
                Json::obj(vec![
                    ("positionEncoding", Json::Str("utf-16".into())),
                    (
                        "textDocumentSync",
                        Json::obj(vec![
                            ("openClose", Json::Bool(true)),
                            ("change", Json::Num(1.0)), // full sync
                        ]),
                    ),
                ]),
            )])),
            "shutdown" => respond(&mut out, Json::Null),
            "exit" => std::process::exit(0),
            "textDocument/didOpen" => {
                if let (Some(uri), Some(text)) = (
                    doc_uri(),
                    params
                        .and_then(|p| p.get("textDocument"))
                        .and_then(|t| t.get("text"))
                        .map(|t| t.s().to_string()),
                ) {
                    let path = uri_to_path(&uri).unwrap_or_else(|| PathBuf::from(&uri));
                    docs.insert(uri.clone(), (path, text));
                    recheck!(out, uri);
                }
            }
            "textDocument/didChange" => {
                // Full sync — the last content change IS the document.
                let text = params
                    .and_then(|p| p.get("contentChanges"))
                    .and_then(|c| match c {
                        Json::Arr(cs) => cs.last().and_then(|c| c.get("text")),
                        _ => None,
                    })
                    .map(|t| t.s().to_string());
                if let (Some(uri), Some(text)) = (doc_uri(), text) {
                    if let Some((_, t)) = docs.get_mut(&uri) {
                        *t = text;
                    }
                    recheck!(out, uri);
                }
            }
            "textDocument/didSave" => {
                // Imports live on disk — a save anywhere can change
                // this doc's diagnostics.
                if let Some(uri) = doc_uri() {
                    recheck!(out, uri);
                }
            }
            "textDocument/didClose" => {
                if let Some(uri) = doc_uri() {
                    docs.remove(&uri);
                    if dirty.remove(&uri) {
                        out.extend(frame(&publish(&uri, vec![])));
                    }
                }
            }
            _ => {
                if let Some(id) = &id {
                    // Unknown REQUEST — keep well-behaved clients from
                    // hanging. Unknown notifications are ignored.
                    out.extend(frame(&Json::obj(vec![
                        ("jsonrpc", Json::Str("2.0".into())),
                        ("id", id.clone()),
                        (
                            "error",
                            Json::obj(vec![
                                ("code", Json::Num(-32601.0)),
                                ("message", Json::Str("method not found".into())),
                            ]),
                        ),
                    ])));
                }
            }
        }
        if !out.is_empty() {
            let mut w = stdout.lock();
            let _ = w.write_all(&out);
            let _ = w.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf16_columns() {
        // ASCII — col == utf16 units.
        assert_eq!(utf16_col("let x = 1;", 5), 4);
        // BMP char — `é` is 1 UTF-16 unit, same as a char.
        assert_eq!(utf16_col("let héllo = 0;", 8), 7);
        // Non-BMP — `🚀` is 2 UTF-16 units: char col 3 lands at 3.
        assert_eq!(utf16_col("a🚀b", 3), 3);
        // Past-end col saturates at the line length.
        assert_eq!(utf16_col("ab", 99), 2);
    }

    #[test]
    fn uri_codec() {
        let p = PathBuf::from("/tmp/a b.lang");
        assert_eq!(path_to_uri(&p), "file:///tmp/a%20b.lang");
        assert_eq!(uri_to_path("file:///tmp/a%20b.lang"), Some(p));
        assert_eq!(uri_to_path("file:///x/y.lang"), Some(PathBuf::from("/x/y.lang")));
        assert_eq!(uri_to_path("notauri"), None);
    }

    #[test]
    fn json_roundtrip() {
        // `\u00e9` = é, `\ud83d\ude80` = 🚀 — the parser must
        // decode the surrogate pair to one char.
        let src = r#"{"a":[1,"x\n",true,null],"b":{"c":-2.5},"u":"\u00e9\ud83d\ude80"}"#;
        let j = parse_json(src.as_bytes()).unwrap();
        assert_eq!(j.get("a").unwrap().get("__nope__"), None);
        assert_eq!(j.get("u").unwrap().s(), "é🚀");
        let mut s = String::new();
        j.write(&mut s);
        // The emitted form parses back to the same value.
        assert_eq!(parse_json(s.as_bytes()), Some(j));
    }
}
