//! The native suite inside `cargo test`: every `native-tests/*.lang`
//! must compile (or fail) and run as its directives declare.

use ourc::driver::ntest;
use std::path::PathBuf;

#[test]
fn native_tests() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("native-tests");
    let (pass, fail, report) = ntest::run_dir(&dir, &Default::default());
    print!("{report}");
    assert!(fail == 0 && pass > 0, "native tests failed");
}
