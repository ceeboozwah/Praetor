// bench/kvsrv — iterative TCP key/value store: one request per
// connection (`SET k v\n` -> `OK`, `GET k\n` -> `VAL v`/`ERR`).
// Same map as the suite: FNV-1a, linear probe, owned key copies.
use std::io::{Read, Write};
use std::net::TcpListener;

#[derive(Clone)]
struct Ent { k: Vec<u8>, hash: u64, v: i64 }

fn fnv(k: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in k { h = (h ^ b as u64).wrapping_mul(0x100000001b3); }
    h
}

struct Map { slots: Vec<Option<Ent>>, len: usize, used: usize }

fn put(m: &mut Map, k: &[u8], h: u64, v: i64) {
    if (m.used + 1) * 10 >= m.slots.len() * 7 { grow(m); }
    let mut i = (h as usize) & (m.slots.len() - 1);
    while let Some(e) = &mut m.slots[i] {
        if e.hash == h && e.k == k { e.v = v; return; }
        i = (i + 1) & (m.slots.len() - 1);
    }
    m.slots[i] = Some(Ent { k: k.to_vec(), hash: h, v });
    m.len += 1;
    m.used += 1;
}

fn grow(m: &mut Map) {
    let ncap = m.slots.len() * 2;
    let old = std::mem::replace(&mut m.slots, vec![None; ncap]);
    m.len = 0;
    m.used = 0;
    for e in old.into_iter().flatten() { put(m, &e.k, e.hash, e.v); }
}

fn get(m: &Map, k: &[u8], h: u64) -> Option<i64> {
    let mut i = (h as usize) & (m.slots.len() - 1);
    loop {
        match &m.slots[i] {
            Some(e) if e.hash == h && e.k == k => return Some(e.v),
            Some(_) => i = (i + 1) & (m.slots.len() - 1),
            None => return None,
        }
    }
}

fn handle(c: &mut std::net::TcpStream, m: &mut Map) {
    let mut buf = [0u8; 1024];
    let n = match c.read(&mut buf) { Ok(n) => n, Err(_) => return };
    if n == 0 { return; }
    let (op, rest) = if buf[..4] == *b"SET " { (1, &buf[4..n]) }
        else if buf[..4] == *b"GET " { (2, &buf[4..n]) }
        else { let _ = c.write_all(b"ERR"); return; };
    let kend = rest.iter().position(|&b| b == b' ' || b == b'\n').unwrap_or(0);
    if kend == 0 { let _ = c.write_all(b"ERR"); return; }
    let key = &rest[..kend];
    let h = fnv(key);
    if op == 1 {
        if kend + 1 >= rest.len() { let _ = c.write_all(b"ERR"); return; }
        let mut v: i64 = 0;
        let mut ok = false;
        for &b in &rest[kend + 1..] {
            if b.is_ascii_digit() { v = v * 10 + (b - b'0') as i64; ok = true; }
            else { break; }
        }
        if !ok { let _ = c.write_all(b"ERR"); return; }
        put(m, key, h, v);
        let _ = c.write_all(b"OK");
    } else {
        match get(m, key, h) {
            Some(v) => { let _ = c.write_all(format!("VAL {}", v).as_bytes()); }
            None => { let _ = c.write_all(b"ERR"); }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 { std::process::exit(1); }
    let nconn: i64 = args[2].parse().unwrap();
    let srv = match TcpListener::bind(format!("127.0.0.1:{}", args[1])) {
        Ok(s) => s,
        Err(_) => std::process::exit(3),
    };
    let mut m = Map { slots: vec![None; 64], len: 0, used: 0 };
    let mut i: i64 = 0;
    while i < nconn {
        let (mut c, _) = match srv.accept() { Ok(x) => x, Err(_) => std::process::exit(5) };
        handle(&mut c, &mut m);
        i += 1;
    }
    println!("{}", i);
}
