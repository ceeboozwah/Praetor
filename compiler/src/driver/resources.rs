//! Toolchain resources embedded in the binary (N61, INSTALL.md §2).
//! `stdlib/*.lang` and `runtime/*.{c,h}` are compiled in by
//! `build.rs` and materialized to `$OUR_HOME/lib/<ver>/` on first
//! use — an installed `our` needs nothing beyond itself. Env vars
//! `OUR_STDLIB`/`OUR_RUNTIME` override for dev and debugging.

use std::path::PathBuf;
use std::sync::OnceLock;

include!(concat!(env!("OUT_DIR"), "/resources.rs"));

/// `~/.our` — overridden by `OUR_HOME` (registry.rs has the same
/// convention for the package cache).
fn our_home() -> PathBuf {
    if let Ok(h) = std::env::var("OUR_HOME") {
        return PathBuf::from(h);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".our")
}

/// The materialized copy of a resource group (`stdlib` or
/// `runtime`): every embedded file, written if missing or stale.
/// Per-group `OnceLock` — the check runs once per process.
fn materialized(group: &str) -> Result<PathBuf, String> {
    static SLOTS: OnceLock<std::sync::Mutex<std::collections::HashMap<String, PathBuf>>> =
        OnceLock::new();
    let map = SLOTS.get_or_init(Default::default);
    if let Some(p) = map.lock().unwrap().get(group) {
        return Ok(p.clone());
    }
    let dir = our_home()
        .join("lib")
        .join(env!("CARGO_PKG_VERSION"))
        .join(group);
    for (name, bytes) in FILES {
        let Some(rel) = name.strip_prefix(&format!("{group}/")) else {
            continue;
        };
        let dst = dir.join(rel);
        let fresh = std::fs::read(&dst).is_ok_and(|old| old == *bytes);
        if !fresh {
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(&dst, bytes).map_err(|e| e.to_string())?;
        }
    }
    map.lock().unwrap().insert(group.to_string(), dir.clone());
    Ok(dir)
}

/// Where `import std.x` resolves — `OUR_STDLIB` wins, else the
/// materialized tree. Replaces the old CARGO_MANIFEST_DIR path.
pub fn stdlib_dir() -> PathBuf {
    if let Ok(d) = std::env::var("OUR_STDLIB") {
        return PathBuf::from(d);
    }
    materialized("stdlib").unwrap_or_else(|e| {
        eprintln!("our: cannot materialize embedded stdlib: {e}");
        std::process::exit(1);
    })
}

/// Where `mmlrt*.c`/`mmlrt.h` live for the link step — `OUR_RUNTIME`
/// wins, else the materialized tree.
pub fn runtime_dir() -> PathBuf {
    if let Ok(d) = std::env::var("OUR_RUNTIME") {
        return PathBuf::from(d);
    }
    materialized("runtime").unwrap_or_else(|e| {
        eprintln!("our: cannot materialize embedded runtime: {e}");
        std::process::exit(1);
    })
}
