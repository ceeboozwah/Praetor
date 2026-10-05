// bench/jsonparse — DOM-parse argv[1] (same JSON subset: objects,
// arrays, no-escape strings, ints, bool, null), count nodes, drop.
use std::fs;

enum J {
    Num(i64),
    Str(Vec<u8>),
    Arr(Vec<J>),
    Obj(Vec<(Vec<u8>, J)>),
    Bool(i32),
    Null,
}

struct Cur<'a> { b: &'a [u8], i: usize }

fn ws(p: &mut Cur) {
    while p.i < p.b.len() && matches!(p.b[p.i], b' ' | b'\n' | b'\t' | b'\r') { p.i += 1; }
}

fn lit(p: &mut Cur, s: &[u8]) -> bool {
    if p.i + s.len() <= p.b.len() && &p.b[p.i..p.i + s.len()] == s {
        p.i += s.len();
        true
    } else {
        false
    }
}

fn jstr(p: &mut Cur) -> Option<Vec<u8>> {
    ws(p);
    if p.i >= p.b.len() || p.b[p.i] != b'"' { return None; }
    p.i += 1;
    let lo = p.i;
    while p.i < p.b.len() && p.b[p.i] != b'"' { p.i += 1; }
    if p.i >= p.b.len() { return None; }
    let v = p.b[lo..p.i].to_vec();
    p.i += 1;
    Some(v)
}

fn jnum(p: &mut Cur) -> Option<J> {
    let mut neg = false;
    if p.i < p.b.len() && p.b[p.i] == b'-' { neg = true; p.i += 1; }
    if p.i >= p.b.len() || !p.b[p.i].is_ascii_digit() { return None; }
    let mut v: i64 = 0;
    while p.i < p.b.len() && p.b[p.i].is_ascii_digit() {
        v = v * 10 + (p.b[p.i] - b'0') as i64;
        p.i += 1;
    }
    Some(J::Num(if neg { -v } else { v }))
}

fn jarr(p: &mut Cur) -> Option<J> {
    p.i += 1;
    let mut v = Vec::new();
    ws(p);
    if p.i < p.b.len() && p.b[p.i] == b']' { p.i += 1; return Some(J::Arr(v)); }
    loop {
        v.push(jparse(p)?);
        ws(p);
        if p.i >= p.b.len() { return None; }
        match p.b[p.i] {
            b',' => p.i += 1,
            b']' => { p.i += 1; return Some(J::Arr(v)); }
            _ => return None,
        }
    }
}

fn jobj(p: &mut Cur) -> Option<J> {
    p.i += 1;
    let mut v = Vec::new();
    ws(p);
    if p.i < p.b.len() && p.b[p.i] == b'}' { p.i += 1; return Some(J::Obj(v)); }
    loop {
        let k = jstr(p)?;
        ws(p);
        if p.i >= p.b.len() || p.b[p.i] != b':' { return None; }
        p.i += 1;
        v.push((k, jparse(p)?));
        ws(p);
        if p.i >= p.b.len() { return None; }
        match p.b[p.i] {
            b',' => p.i += 1,
            b'}' => { p.i += 1; return Some(J::Obj(v)); }
            _ => return None,
        }
    }
}

fn jparse(p: &mut Cur) -> Option<J> {
    ws(p);
    if p.i >= p.b.len() { return None; }
    let c = p.b[p.i];
    if c == b'{' { return jobj(p); }
    if c == b'[' { return jarr(p); }
    if c == b'"' { return Some(J::Str(jstr(p)?)); }
    if c == b't' && lit(p, b"true") { return Some(J::Bool(1)); }
    if c == b'f' && lit(p, b"false") { return Some(J::Bool(0)); }
    if c == b'n' && lit(p, b"null") { return Some(J::Null); }
    if c == b'-' || c.is_ascii_digit() { return jnum(p); }
    None
}

fn count(j: &J) -> u64 {
    1 + match j {
        J::Arr(v) => v.iter().map(count).sum(),
        J::Obj(v) => v.iter().map(|(_, x)| count(x)).sum(),
        _ => 0,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 { std::process::exit(1); }
    let buf = match fs::read(&args[1]) { Ok(b) => b, Err(_) => std::process::exit(2) };
    let mut p = Cur { b: &buf, i: 0 };
    let r = match jparse(&mut p) { Some(r) => r, None => std::process::exit(3) };
    println!("{}", count(&r));
}
