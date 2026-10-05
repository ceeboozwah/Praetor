// bench/wordfreq — count tokens + unique words over argv[1].
// Open-addressing table matching the lab StrMap: FNV-1a, linear
// probe, stored hashes, grow at 70%. Keys are zero-copy &str views
// into the file buffer — same as the .lang side.
use std::fs;

#[derive(Clone, Copy)]
struct Ent<'a> { k: &'a str, hash: u64, v: i64 }

fn fnv(k: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in k.as_bytes() { h = (h ^ b as u64).wrapping_mul(0x100000001b3); }
    h
}

struct Map<'a> { slots: Vec<Option<Ent<'a>>>, len: usize, used: usize }

fn insert<'a>(m: &mut Map<'a>, k: &'a str, h: u64, v: i64) {
    if (m.used + 1) * 10 >= m.slots.len() * 7 { grow(m); }
    let mut i = (h as usize) & (m.slots.len() - 1);
    while let Some(e) = &mut m.slots[i] {
        if e.hash == h && e.k == k { e.v = v; return; }
        i = (i + 1) & (m.slots.len() - 1);
    }
    m.slots[i] = Some(Ent { k, hash: h, v });
    m.len += 1;
    m.used += 1;
}

fn grow<'a>(m: &mut Map<'a>) {
    let ncap = m.slots.len() * 2;
    let old = std::mem::replace(&mut m.slots, vec![None; ncap]);
    m.len = 0;
    m.used = 0;
    for e in old.into_iter().flatten() { insert(m, e.k, e.hash, e.v); }
}

fn get<'a>(m: &mut Map<'a>, k: &'a str, h: u64) -> Option<usize> {
    let mut i = (h as usize) & (m.slots.len() - 1);
    loop {
        match &m.slots[i] {
            Some(e) if e.hash == h && e.k == k => return Some(i),
            Some(_) => i = (i + 1) & (m.slots.len() - 1),
            None => return None,
        }
    }
}

fn is_ws(c: u8) -> bool { c == b' ' || c == b'\n' || c == b'\r' || c == b'\t' }

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 { std::process::exit(1); }
    let buf = match fs::read(&args[1]) { Ok(b) => b, Err(_) => std::process::exit(2) };

    let mut m = Map { slots: vec![None::<Ent>; 16], len: 0, used: 0 };
    let mut toks: i64 = 0;
    let n = buf.len();
    let mut wlo = 0usize;
    let mut in_word = false;
    for i in 0..=n {
        let closing = i == n || is_ws(buf[i]);
        if in_word && closing {
            let k = unsafe { std::str::from_utf8_unchecked(&buf[wlo..i]) };
            let h = fnv(k);
            match get(&mut m, k, h) {
                Some(i) => m.slots[i].as_mut().unwrap().v += 1,
                None => insert(&mut m, k, h, 1),
            }
            toks += 1;
            in_word = false;
        }
        if !in_word && !closing {
            wlo = i;
            in_word = true;
        }
    }
    println!("{} {}", toks, m.len);
}
