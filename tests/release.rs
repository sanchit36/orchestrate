mod common;

use common::Scratch;

#[test]
fn version_is_the_crate_version() {
    let s = Scratch::new();

    let want = format!("orchestrate {}\n", env!("CARGO_PKG_VERSION"));
    assert_eq!(s.run(&["--version"]).ok(), want);
}
