#[test]
fn ui_compile_fail_tests() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/compile_fail/*.rs");
}
