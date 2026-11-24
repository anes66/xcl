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
fn malformed_journal_headers_fail_before_gpu_or_progress_creation() {
    use std::fs;
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("xcl-invalid-journal-{}-{nonce}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    for (index, (header, watch)) in [
        (&b"not an XCL journal"[..], false), (&b"XCLPJ000"[..], true),
        (&b"XCL"[..], false), (&b""[..], false),
    ].into_iter().enumerate() {
        let path = dir.join(format!("journal-{index}.bin"));
        fs::write(&path, header).unwrap();
        let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_xcl"));
        cmd.arg("pipelines").arg(&path).env("XCL_ADAPTER", "xcl-header-test-no-matching-adapter");
        if watch { cmd.arg("--watch"); }
        let out = cmd.output().unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{out:?}");
        assert!(out.stdout.is_empty() && stderr.contains("invalid pipeline journal header"), "{out:?}");
        assert!(!path.with_extension("done").exists() && !path.with_extension("trying").exists());
    }
    fs::remove_dir_all(dir).unwrap();
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
