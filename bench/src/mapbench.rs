// bench/mapbench — insert N u64 keys, look them all up.
// Same table as StrMap: FNV-1a on the key bytes, linear probe,
// stored hash, grow at 70%.
#[derive(Clone, Copy)]
struct Ent { k: u64, hash: u64, v: u64 }

fn fnv64(mut k: u64) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for _ in 0..8 {
        h = (h ^ (k & 0xff)).wrapping_mul(0x100000001b3);
        k >>= 8;
    }
    h
}

struct Map { slots: Vec<Ent>, len: usize, used: usize }

fn insert(m: &mut Map, k: u64, h: u64, v: u64) {
    if (m.used + 1) * 10 >= m.slots.len() * 7 { grow(m); }
    let mut i = (h as usize) & (m.slots.len() - 1);
    while m.slots[i].v != 0 {
        if m.slots[i].hash == h && m.slots[i].k == k { m.slots[i].v = v; return; }
        i = (i + 1) & (m.slots.len() - 1);
    }
    m.slots[i] = Ent { k, hash: h, v };
    m.len += 1;
    m.used += 1;
}

fn grow(m: &mut Map) {
    let ncap = m.slots.len() * 2;
    let old = std::mem::replace(&mut m.slots, vec![Ent { k: 0, hash: 0, v: 0 }; ncap]);
    m.len = 0;
    m.used = 0;
    for e in old { if e.v != 0 { insert(m, e.k, e.hash, e.v); } }
}

fn get(m: &Map, k: u64, h: u64) -> u64 {
    let mut i = (h as usize) & (m.slots.len() - 1);
    while m.slots[i].v != 0 {
        if m.slots[i].hash == h && m.slots[i].k == k { return m.slots[i].v; }
        i = (i + 1) & (m.slots.len() - 1);
    }
    0
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 { std::process::exit(1); }
    let n: u64 = args[1].parse().unwrap();
    let mut m = Map { slots: vec![Ent { k: 0, hash: 0, v: 0 }; 16], len: 0, used: 0 };
    for i in 1..=n {
        let k = i.wrapping_mul(0x9e3779b97f4a7c15);
        insert(&mut m, k, fnv64(k), i);
    }
    let mut sum: u64 = 0;
    for i in 1..=n {
        let k = i.wrapping_mul(0x9e3779b97f4a7c15);
        sum += get(&m, k, fnv64(k));
    }
    println!("{} {}", m.len, sum);
}
