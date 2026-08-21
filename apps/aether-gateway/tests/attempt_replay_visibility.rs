#[test]
fn sibling_modules_cannot_mint_replay_authority_or_capabilities() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/attempt_replay_bypass.rs");
}
