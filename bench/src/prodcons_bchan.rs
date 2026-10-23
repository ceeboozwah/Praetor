// bench/prodcons_bchan — N heap-allocated packets through a BOUNDED
// channel (capacity argv[2]), producer thread -> main consumer.
// Box ownership crosses the send; the consumer drops each packet.
// Track-B idiomatic Rust: sync_channel(cap) is the stdlib bounded
// channel — the exact std facility for bounded producer-consumer.
use std::sync::mpsc::sync_channel;
use std::thread;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 { std::process::exit(1); }
    let n: i64 = args[1].parse().unwrap();
    let cap: usize = args[2].parse().unwrap();
    let (tx, rx) = sync_channel::<Box<i64>>(cap);
    let t = thread::spawn(move || {
        for i in 0..n { tx.send(Box::new(i)).unwrap(); }
        tx.send(Box::new(-1)).unwrap();
    });
    let mut total: i64 = 0;
    loop {
        let g = rx.recv().unwrap();
        if *g < 0 { break; }
        total += *g;
    }
    t.join().unwrap();
    println!("{}", total);
}
