//! QA-002 — runtime allocator boundary tests. `mml_alloc_slice`'s
//! `size * count` used to be evaluated before the overflow check —
//! signed-overflow UB at the boundary — and neither entry point
//! rejected negative sizes before the `size_t` cast. Hostile
//! operands must now abort BEFORE the multiply; the test compiles
//! `mmlrt_common.c` with the host C compiler — the same toolchain
//! every `our` build already requires — and runs each case as a
//! fresh process so `abort()` is an observable outcome.

use std::path::PathBuf;
use std::process::Command;

const DRIVER: &str = r#"
#include <limits.h>
#include <stdlib.h>
void *mml_alloc(long long);
void *mml_alloc_slice(long long, long long);
int main(int argc, char **argv) {
    int c = argc > 1 ? atoi(argv[1]) : -1;
    switch (c) {
        case 0: return mml_alloc_slice(4, 8) ? 0 : 1;
        case 1: return mml_alloc_slice(0, 0) ? 0 : 1;
        case 2: return mml_alloc_slice(0, LLONG_MAX) ? 0 : 1;
        case 3: mml_alloc_slice(-1, 4); return 2;
        case 4: mml_alloc_slice(4, -1); return 2;
        case 5: mml_alloc_slice(LLONG_MAX, 2); return 2;
        case 6: mml_alloc_slice(LLONG_MAX / 2 + 1, 2); return 2;
        case 7: mml_alloc(-1); return 2;
        default: return 9;
    }
}
"#;

#[cfg(not(target_os = "windows"))]
#[test]
fn alloc_slice_boundary_battery() {
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtime");
    let w = std::env::temp_dir().join(format!("ourc-qa002-{}", std::process::id()));
    std::fs::create_dir_all(&w).unwrap();
    let drv = w.join("drv.c");
    let bin = w.join("drv");
    std::fs::write(&drv, DRIVER).unwrap();
    let cc = Command::new("cc")
        .args(["-Wall", "-Werror", "-O1"])
        .arg(&drv)
        .arg(src.join("mmlrt_common.c"))
        .arg("-o")
        .arg(&bin)
        .output()
        .expect("spawn cc");
    assert!(
        cc.status.success(),
        "cc build failed: {}",
        String::from_utf8_lossy(&cc.stderr)
    );

    // (case, expect_abort, stderr-fragment)
    let cases: &[(i32, bool, &str)] = &[
        (0, false, ""),
        (1, false, ""),
        (2, false, ""),
        (3, true, "negative operand"),
        (4, true, "negative operand"),
        (5, true, "size overflow"),
        (6, true, "size overflow"),
        (7, true, "size -1 is negative"),
    ];
    for (case, want_abort, frag) in cases {
        let r = Command::new(&bin)
            .arg(case.to_string())
            .output()
            .expect("spawn driver");
        let err = String::from_utf8_lossy(&r.stderr);
        if *want_abort {
            assert!(
                !r.status.success() && err.contains(frag),
                "case {case}: expected abort `{frag}`, got {}",
                err.trim()
            );
        } else {
            assert!(
                r.status.success(),
                "case {case}: expected clean alloc, got {}",
                err.trim()
            );
        }
    }
    let _ = std::fs::remove_dir_all(&w);
}
