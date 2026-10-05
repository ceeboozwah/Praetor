// bench/psum — parallel sum over N i64s. Four workers; each
// allocates, fills, sums, and frees its own quarter.
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::thread;

fn quarter(lo: i64, hi: i64) -> i64 {
    let n = (hi - lo) as usize;
    let mut b = vec![0i64; n];
    for i in 0..n { b[i] = lo + i as i64; }
    b.iter().sum()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 { std::process::exit(1); }
    let n: i64 = args[1].parse().unwrap();
    let q = n / 4;
    let acc = Arc::new(AtomicI64::new(0));
    let bounds = [0, q, 2 * q, 3 * q, n];
    let mut ts = Vec::new();
    for i in 0..4 {
        let acc = Arc::clone(&acc);
        let (lo, hi) = (bounds[i], bounds[i + 1]);
        ts.push(thread::spawn(move || {
            acc.fetch_add(quarter(lo, hi), Ordering::Relaxed);
        }));
    }
    for t in ts { t.join().unwrap(); }
    println!("{}", acc.load(Ordering::Relaxed));
}
