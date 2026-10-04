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

/// N36B — hover + definition consume the checker's resolved-symbol
/// index: locals, calls, fields, and module crossings.
#[test]
fn lsp_hover_and_definition() {
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
    assert_eq!(init["result"]["capabilities"]["hoverProvider"], true);
    assert_eq!(init["result"]["capabilities"]["definitionProvider"], true);

    let uri = "file:///tmp/ourc_lsp_hover.lang";
    let src = "fn add(own x: i32, y: i32) -> i32 {\n    return x + y;\n}\nfn main() -> i32 {\n    let v = add(1, 2);\n    return v;\n}\n";
    framed(&mut w, &json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{
        "textDocument":{"uri":uri,"languageId":"our","version":1,"text":src}}}));
    // A clean doc publishes nothing (there was never a dirty set) —
    // the next request-response proves didOpen was processed.

    // Hover on `v` at `return v` (0-based line 5, char 11) — the
    // resolved type, not a guess.
    framed(&mut w, &json!({"jsonrpc":"2.0","id":2,"method":"textDocument/hover","params":{
        "textDocument":{"uri":uri},"position":{"line":5,"character":11}}}));
    let h = recv(&rx);
    assert_eq!(h["result"]["contents"]["value"], "`v`: i32");

    // Hover on `add` at the call site — the full resolved signature.
    framed(&mut w, &json!({"jsonrpc":"2.0","id":3,"method":"textDocument/hover","params":{
        "textDocument":{"uri":uri},"position":{"line":4,"character":13}}}));
    let h = recv(&rx);
    assert_eq!(
        h["result"]["contents"]["value"],
        "fn add(own x: i32, y: i32) -> i32"
    );

    // Definition on `add` → the `fn` decl on line 0.
    framed(&mut w, &json!({"jsonrpc":"2.0","id":4,"method":"textDocument/definition","params":{
        "textDocument":{"uri":uri},"position":{"line":4,"character":13}}}));
    let d = recv(&rx);
    assert_eq!(d["result"]["uri"], uri);
    assert_eq!(d["result"]["range"]["start"], json!({"line":0,"character":0}));

    // Definition on `v` → the `let` decl on line 4.
    framed(&mut w, &json!({"jsonrpc":"2.0","id":5,"method":"textDocument/definition","params":{
        "textDocument":{"uri":uri},"position":{"line":5,"character":11}}}));
    let d = recv(&rx);
    assert_eq!(d["result"]["range"]["start"], json!({"line":4,"character":4}));

    // Whitespace → no occurrence → null, not an error.
    framed(&mut w, &json!({"jsonrpc":"2.0","id":6,"method":"textDocument/hover","params":{
        "textDocument":{"uri":uri},"position":{"line":5,"character":0}}}));
    assert_eq!(recv(&rx)["result"], Value::Null);

    framed(&mut w, &json!({"jsonrpc":"2.0","id":9,"method":"shutdown","params":null}));
    assert_eq!(recv(&rx)["result"], Value::Null);
    framed(&mut w, &json!({"jsonrpc":"2.0","method":"exit"}));
    assert!(p.wait().unwrap().success());
}

/// N36C — completion from the resolved-symbol index: locals, module
/// members, fields/methods through a receiver's resolved type, and
/// candidates that survive a broken buffer.
#[test]
fn lsp_completion() {
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
    assert_eq!(
        init["result"]["capabilities"]["completionProvider"]["triggerCharacters"],
        json!([".", ":"])
    );

    let uri = "file:///tmp/ourc_lsp_comp.lang";
    // Deliberately broken (`bad`): completion must still answer.
    let src = "struct Pt { x: i32 }\nfn add(own x: i32, y: i32) -> i32 {\n    return x + y;\n}\nfn main() -> i32 {\n    let p = Pt { x: 1 };\n    let v = add(1, 2);\n    let bad: i32 = \"nope\";\n    return p.x;\n}\n";
    framed(&mut w, &json!({"jsonrpc":"2.0","method":"textDocument/didOpen","params":{
        "textDocument":{"uri":uri,"languageId":"our","version":1,"text":src}}}));
    let _ = diag_msgs(&rx, uri); // the type error publishes; occs survive

    let comp = |w: &mut ChildStdin,
                rx: &std::sync::mpsc::Receiver<Value>,
                id: i64,
                line: i64,
                ch: i64| {
        framed(w, &json!({"jsonrpc":"2.0","id":id,"method":"textDocument/completion","params":{
            "textDocument":{"uri":uri},"position":{"line":line,"character":ch}}}));
        let r = recv(rx);
        r["result"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                (
                    i["label"].as_str().unwrap().to_string(),
                    i["kind"].as_i64().unwrap(),
                )
            })
            .collect::<Vec<(String, i64)>>()
    };

    // Top-level inside main: `v`/`p` (locals) + `add`/`main`/`Pt`
    // (module decls); `add`'s params must NOT leak from `add`.
    let items = comp(&mut w, &rx, 2, 7, 3);
    assert!(items.contains(&("v".into(), 6)), "local: {items:?}");
    assert!(items.contains(&("p".into(), 6)), "local: {items:?}");
    assert!(items.contains(&("add".into(), 3)), "fn: {items:?}");
    assert!(items.contains(&("Pt".into(), 22)), "struct: {items:?}");
    assert!(
        !items.iter().any(|(l, _)| l == "x"),
        "other fn's params leak: {items:?}"
    );

    // `return p.` — members through the receiver's resolved struct type.
    let items = comp(&mut w, &rx, 3, 8, 13);
    assert_eq!(items, vec![("x".to_string(), 5)], "fields: {items:?}");

    framed(&mut w, &json!({"jsonrpc":"2.0","id":9,"method":"shutdown","params":null}));
    assert_eq!(recv(&rx)["result"], Value::Null);
    framed(&mut w, &json!({"jsonrpc":"2.0","method":"exit"}));
    assert!(p.wait().unwrap().success());
}
