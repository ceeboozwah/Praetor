// bench/prodcons — N heap-allocated i64 packets through an
// unbounded channel, producer thread -> main consumer. Box
// ownership crosses the send; the consumer drops each packet.
use std::sync::mpsc::channel;
use std::thread;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 2 { std::process::exit(1); }
    let n: i64 = args[1].parse().unwrap();
    let (tx, rx) = channel::<Box<i64>>();
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
