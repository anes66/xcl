//! Command help must work before resolving a title or selecting a GPU.

#[test]
fn pipelines_help_needs_neither_a_target_nor_a_matching_adapter() {
    for args in [
        &["pipelines", "--help"][..],
        &["pipelines", "-h"][..],
        &["pipelines", "__xcl_missing_help_journal__.bin", "--help"][..],
        &["pipelines", "__xcl_missing_help_journal__.bin", "-h"][..],
    ] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_xcl"))
            .args(args).env("XCL_ADAPTER", "xcl-help-test-no-matching-adapter").output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{args:?}: {stdout}\n{stderr}");
        assert!(stdout.starts_with("xcl pipelines ") && stdout.contains("--threads"), "{args:?}: {stdout}");
        assert!(stderr.is_empty(), "{args:?}: {stderr}");
    }
}
