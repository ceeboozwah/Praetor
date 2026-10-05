// bench/fcopy — copy argv[1] to argv[2], 64KB chunks.
use std::fs::File;
use std::io::{Read, Write};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 { std::process::exit(1); }
    let mut src = match File::open(&args[1]) { Ok(f) => f, Err(_) => std::process::exit(2) };
    let mut dst = match File::create(&args[2]) { Ok(f) => f, Err(_) => std::process::exit(3) };
    let mut buf = [0u8; 65536];
    let mut total: i64 = 0;
    loop {
        let n = match src.read(&mut buf) { Ok(n) => n, Err(_) => std::process::exit(4) };
        if n == 0 { break; }
        if dst.write_all(&buf[..n]).is_err() { std::process::exit(5); }
        total += n as i64;
    }
    println!("{}", total);
}
