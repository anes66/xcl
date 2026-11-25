//! Runs the actual build script against source copies; no translator or GPU is invoked.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let root = std::env::temp_dir();
        if self.0.starts_with(&root) && self.0.file_name().unwrap().to_string_lossy().starts_with("xcl-source-identity-") {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

fn copy_sources(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_sources(&path, &target);
        } else if path.extension().is_some_and(|e| e == "rs") {
            std::fs::copy(path, target).unwrap();
        }
    }
}

fn identities(probe: &Path) -> BTreeMap<String, String> {
    let out = Command::new(probe).env("CARGO_CFG_TARGET_ENV", "msvc").output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let ids: BTreeMap<_, _> = String::from_utf8(out.stdout).unwrap().lines()
        .filter_map(|line| line.strip_prefix("cargo:rustc-env="))
        .map(|line| { let (k, v) = line.split_once('=').unwrap(); (k.to_string(), v.to_string()) }).collect();
    assert_eq!(ids.len(), 3);
    ids
}

#[test]
fn source_identity_tracks_runtime_files_but_not_standalone_tests() {
    const COMMON: &str = "XCL_TRANSLATOR_SOURCE_ID";
    const DIRECT: &str = "XCL_DIRECT_DXIL_SOURCE_ID";
    const LANES: &str = "XCL_PER_LANE_SOURCE_ID";
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let unique = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let scratch = Scratch(std::env::temp_dir().join(format!("xcl-source-identity-{}-{unique}", std::process::id())));
    let root = &scratch.0;
    copy_sources(&manifest.join("src"), &root.join("src"));
    let probe = root.join(format!("probe{}", std::env::consts::EXE_SUFFIX));
    let out = Command::new("rustc").arg("--edition=2021").arg(manifest.join("build.rs"))
        .arg("-o").arg(&probe).env("CARGO_MANIFEST_DIR", root).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let baseline = identities(&probe);
    let changed = |ids: &BTreeMap<String, String>, key: &str| {
        for name in [COMMON, DIRECT, LANES] {
            assert_eq!(ids[name] != baseline[name], name == key, "identity {name}, expected change to {key}");
        }
    };
    for (file, expected) in [("gpu/dxil/cfg.rs", DIRECT), ("gpu/dxil/module/opt.rs", DIRECT), ("gpu/recomp/lower.rs", LANES), ("gpu/recomp/emit.rs", COMMON), ("gpu/dxil/ops_tests.rs", "")] {
        let path = root.join("src").join(file);
        let original = std::fs::read(&path).unwrap();
        let mut edited = original.clone();
        edited.extend_from_slice(b"\n// source identity probe\n");
        std::fs::write(&path, edited).unwrap();
        changed(&identities(&probe), expected);
        std::fs::write(&path, original).unwrap();
        assert_eq!(identities(&probe), baseline);
    }
    let a = root.join("src/gpu/dxil/probe_added.rs");
    let z = root.join("src/gpu/dxil/module/probe_nested.rs");
    std::fs::write(&a, b"// synthetic new runtime source\n").unwrap();
    changed(&identities(&probe), DIRECT);
    std::fs::write(&z, b"// synthetic nested runtime source\n").unwrap();
    let added = identities(&probe);
    changed(&added, DIRECT);
    std::fs::remove_file(&a).unwrap();
    std::fs::remove_file(&z).unwrap();
    std::fs::write(&z, b"// synthetic nested runtime source\n").unwrap();
    std::fs::write(&a, b"// synthetic new runtime source\n").unwrap();
    assert_eq!(identities(&probe), added, "directory creation order changed the identity");
    let renamed = a.with_file_name("probe_renamed.rs");
    std::fs::rename(&a, &renamed).unwrap();
    assert_ne!(identities(&probe)[DIRECT], added[DIRECT], "runtime path is part of the identity");
    std::fs::remove_file(&renamed).unwrap();
    std::fs::remove_file(&z).unwrap();
    assert_eq!(identities(&probe), baseline, "removing probe files restores the identity");
    let text = root.join("src/gpu/dxil/probe.txt");
    std::fs::write(&text, b"not Rust source").unwrap();
    assert_eq!(identities(&probe), baseline);
}
