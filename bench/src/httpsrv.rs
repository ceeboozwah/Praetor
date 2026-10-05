// bench/httpsrv — iterative HTTP responder: accept argv-bounded
// connections, one read, one fixed response, close.
use std::io::{Read, Write};
use std::net::TcpListener;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 { std::process::exit(1); }
    let nconn: i64 = args[2].parse().unwrap();
    let srv = match TcpListener::bind(format!("127.0.0.1:{}", args[1])) {
        Ok(s) => s,
        Err(_) => std::process::exit(3),
    };
    let resp = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
    let mut buf = [0u8; 2048];
    let mut i: i64 = 0;
    while i < nconn {
        let (mut c, _) = match srv.accept() { Ok(x) => x, Err(_) => std::process::exit(5) };
        if c.read(&mut buf).unwrap_or(0) > 0 {
            let _ = c.write_all(resp);
        }
        i += 1;
    }
    println!("{}", i);
}
