mod common;

use common::Scratch;

#[test]
fn version_starts_at_0_1_0() {
    let s = Scratch::new();

    assert_eq!(s.run(&["--version"]).ok(), "orchestrate 0.1.0\n");
}
