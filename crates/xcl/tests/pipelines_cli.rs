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

#[test]
fn invalid_pipeline_options_fail_before_resolving_a_target_or_gpu() {
    for args in [
        &["--limit"][..], &["--limit", "bad"][..], &["--limit", "-1"][..],
        &["--limit", "99999999999999999999999999999"][..],
        &["--threads"][..], &["--threads", "bad"][..], &["--threads", "0"][..],
        &["--threads", "1.5"][..], &["--unknown"][..], &["unexpected"][..],
    ] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_xcl"))
            .args(["pipelines", "__xcl_missing_options_journal__.bin"]).args(args)
            .env("XCL_ADAPTER", "xcl-options-test-no-matching-adapter").output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {stdout}\n{stderr}");
        assert!(stdout.is_empty() && stderr.contains(args[0]), "{args:?}: {stdout}\n{stderr}");
    }
}
