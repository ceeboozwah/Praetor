//! N36A integration: drive `ourc lsp` as a subprocess over framed
//! JSON-RPC — initialize, didOpen (borrow error), didChange (fix →
//! diagnostics clear), shutdown/exit.

use serde_json::{json, Value};
use std::io::Write;
use std::process::{Child, ChildStdin, Command, Stdio};

fn framed(w: &mut ChildStdin, v: &Value) {
    let body = serde_json::to_vec(v).unwrap();
    write!(w, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
    w.write_all(&body).unwrap();
    w.flush().unwrap();
}

/// Reader thread → channel; `recv` with a timeout instead of blocking
/// forever on a wedged server.
fn spawn_reader(mut out: impl std::io::Read + Send + 'static) -> std::sync::mpsc::Receiver<Value> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        loop {
            let mut header = Vec::new();
            let mut byte = [0u8; 1];
            loop {
                if out.read(&mut byte).unwrap_or(0) == 0 {
                    return;
                }
                header.push(byte[0]);
                if header.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let h = String::from_utf8_lossy(&header);
            let len: usize = h
                .split("\r\n")
                .find_map(|l| l.strip_prefix("Content-Length: "))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
            let mut body = vec![0u8; len];
            if out.read_exact(&mut body).is_err() {
                return;
            }
            if tx.send(serde_json::from_slice(&body).unwrap()).is_err() {
                return;
            }
        }
    });
    rx
}

fn recv(rx: &std::sync::mpsc::Receiver<Value>) -> Value {
    rx.recv_timeout(std::time::Duration::from_secs(15))
        .expect("LSP server did not respond")
}

/// Collect publishDiagnostics until `uri` has a set; returns them.
fn diag_msgs(rx: &std::sync::mpsc::Receiver<Value>, uri: &str) -> Vec<Value> {
    for _ in 0..20 {
        let m = recv(rx);
        if m["method"] == "textDocument/publishDiagnostics" && m["params"]["uri"] == uri {
            return m["params"]["diagnostics"].as_array().unwrap().clone();
        }
    }
    panic!("no publishDiagnostics for {uri}");
}

#[test]
fn lsp_publishes_and_cleans_diagnostics() {
    let mut p: Child = Command::new(env!("CARGO_BIN_EXE_ourc"))
        .arg("lsp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut w = p.stdin.take().unwrap();
    let rx = spawn_reader(p.stdout.take().unwrap());

    framed(&mut w, &json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}));
    let init = recv(&rx);
    assert_eq!(init["result"]["capabilities"]["positionEncoding"], "utf-16");

    let uri = "file:///tmp/ourc_lsp_it.lang";
    let bad = "fn main() -> i32 {\n    let heap = system_allocator();\n    let m = alloc<i32>(heap);\n    let r = &*m;\n    free(m);\n    return *r;\n}\n";
    framed(&mut w, &json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{
        "textDocument":{"uri":uri,"languageId":"our","version":1,"text":bad}}}));

    let ds = diag_msgs(&rx, uri);
    assert_eq!(ds.len(), 1, "one borrow conflict");
    let d = &ds[0];
    assert_eq!(d["code"], "E1001");
    assert_eq!(d["severity"], 1);
    assert_eq!(d["source"], "ourc");
    // 0-based: `free(m)` at source line 5 → line 4; `m` col 10 → 9.
    assert_eq!(d["range"]["start"], json!({"line":4,"character":9}));
    // Borrow birth site as relatedInformation.
    let rel = &d["relatedInformation"][0];
    assert!(rel["message"].as_str().unwrap().contains("borrows it here"));
    assert_eq!(rel["location"]["uri"], uri);
    assert_eq!(rel["location"]["range"]["start"], json!({"line":3,"character":12}));

    // Fix → the diagnostic clears with an explicit empty publish.
    framed(&mut w, &json!({"jsonrpc":"2.0","method":"textDocument/didChange","params":{
        "textDocument":{"uri":uri,"version":2},
        "contentChanges":[{"text":"fn main() -> i32 {\n    return 0;\n}\n"}]}}));
    let ds = diag_msgs(&rx, uri);
    assert!(ds.is_empty(), "fixed doc publishes an empty set");

    framed(&mut w, &json!({"jsonrpc":"2.0","id":9,"method":"shutdown","params":null}));
    assert_eq!(recv(&rx)["result"], Value::Null);
    framed(&mut w, &json!({"jsonrpc":"2.0","method":"exit"}));
    assert!(p.wait().unwrap().success());
}
