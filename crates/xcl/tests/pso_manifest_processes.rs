use std::{fs, path::Path, process::Command};

fn run(probe: &Path, cache: &Path, enabled: Option<&str>, small: bool) {
    let mut cmd = Command::new(probe);
    cmd.env("MANIFEST_TEST_CACHE", cache).env_remove("XCL_PSO_MANIFEST");
    if let Some(value) = enabled { cmd.env("XCL_PSO_MANIFEST", value); }
    if small { cmd.arg("small"); }
    let out = cmd.output().expect("run manifest probe");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

fn check_records(path: &Path) {
    let text = fs::read_to_string(path).unwrap();
    let mut lines = text.lines();
    assert!(lines.next().unwrap().starts_with("# xcl pso manifest v1: "));
    let mut next = [0u64; 8];
    let mut previous_ms = 0;
    for (seq, line) in lines.enumerate() {
        let fields: Vec<_> = line.split('\t').collect();
        assert_eq!(fields[0].parse::<usize>().unwrap(), seq, "file sequence: {line}");
        let ms = fields[1].parse::<u128>().unwrap();
        assert!(ms >= previous_ms, "time went backwards: {line}");
        previous_ms = ms;
        let id = match fields[2] {
            "use" => { assert_eq!(fields.len(), 6); u64::from_str_radix(fields[3], 16).unwrap() }
            "build" => { assert_eq!(fields.len(), 9); u64::from_str_radix(fields[5], 16).unwrap() }
            "present" => { assert_eq!(fields.len(), 4); fields[3].parse().unwrap() }
            other => panic!("unexpected event {other}"),
        };
        let thread = (id / 128) as usize;
        let index = id % 128;
        assert_eq!(index, next[thread], "per-thread order: {line}");
        next[thread] += 1;
        match index % 3 {
            0 => {
                assert_eq!(fields[2], "use");
                assert_eq!(fields[3], format!("{id:016x}"));
                assert_eq!(fields[4], if thread % 2 == 0 { "compute" } else { "graphics" });
                assert_eq!(fields[5], if index % 2 == 0 { "ready" } else { "cold" });
            }
            1 => {
                assert_eq!(fields[2], "build");
                assert_eq!(fields[3], "compute");
                assert_eq!(fields[4], (4096 + id).to_string());
                assert_eq!(fields[5], format!("{id:016x}"));
                assert_eq!(fields[6], if index % 2 == 0 { "blob" } else { "cold" });
                assert_eq!(fields[7], (id * 2).to_string());
                assert_eq!(fields[8], (id * 3).to_string());
            }
            _ => assert_eq!(fields[2], "present"),
        }
    }
    assert_eq!(next, [128; 8], "missing or duplicated records");
}

#[test]
fn manifest_processes_preserve_order_and_file_lifecycle() {
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let root = std::env::temp_dir().join(format!("xcl-manifest-{}-{nonce}", std::process::id()));
    fs::create_dir(&root).unwrap();
    let source = root.join("probe.rs");
    let probe = root.join("probe.exe");
    let module = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/hle/d3d12x/pso_manifest.rs");
    // Compile the real module without cfg(test), which deliberately disables its file writer.
    fs::write(&source, format!(r#"
#[macro_export]
macro_rules! xlog_info {{ ($($arg:tt)*) => {{ eprintln!($($arg)*); }}; }}
#[macro_export]
macro_rules! xlog_warn {{ ($($arg:tt)*) => {{ eprintln!($($arg)*); }}; }}
mod gpu {{ pub mod recomp {{ pub mod dxc {{
    pub fn cache_dir() -> std::path::PathBuf {{ std::env::var_os("MANIFEST_TEST_CACHE").unwrap().into() }}
}} }} }}
#[path = {module:?}]
mod manifest;
use manifest::{{note, Event}};
fn main() {{
    if std::env::args().any(|a| a == "small") {{
        note(Event::Use {{ key: 7, compute: true, ready: false }});
        return;
    }}
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let threads: Vec<_> = (0..8).map(|t| {{
        let barrier = barrier.clone();
        std::thread::spawn(move || {{
            barrier.wait();
            for i in 0..128 {{
                let id = t * 128 + i;
                note(match i % 3 {{
                    0 => Event::Use {{ key: id, compute: t % 2 == 0, ready: i % 2 == 0 }},
                    1 => Event::Build {{ kind: "compute", bytes: (4096 + id) as usize,
                        record: Some((id, i % 2 == 0)), driver_ms: (id * 2) as u128, waited_ms: (id * 3) as u128 }},
                    _ => Event::Present {{ n: id }},
                }});
                std::thread::yield_now();
            }}
        }})
    }}).collect();
    for thread in threads {{ thread.join().unwrap(); }}
}}
"#, module = module.to_str().unwrap())).unwrap();
    let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let out = Command::new(compiler).args(["--edition=2021", "--crate-name=manifest_probe"])
        .arg(&source).arg("-o").arg(&probe).output().expect("compile manifest probe");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    let cache = root.join("cache");
    let file = cache.join("pso-manifest.txt");
    for disabled in [None, Some("0"), Some("true")] {
        run(&probe, &cache, disabled, false);
        assert!(!cache.exists(), "disabled recording created a directory");
    }
    for enabled in ["1", " 1 "] {
        run(&probe, &cache, Some(enabled), false);
        check_records(&file);
    }
    let saved = fs::read(&file).unwrap();
    run(&probe, &cache, None, true);
    assert_eq!(fs::read(&file).unwrap(), saved, "disabled process changed an existing file");
    run(&probe, &cache, Some("1"), true);
    let text = fs::read_to_string(&file).unwrap();
    assert_eq!(text.lines().count(), 2, "new process must truncate the previous run");
    let fields: Vec<_> = text.lines().nth(1).unwrap().split('\t').collect();
    assert_eq!(fields[0], "0", "sequence must restart in a new process");
    assert_eq!(&fields[2..], &["use", "0000000000000007", "compute", "cold"]);

    let blocked = root.join("blocked");
    fs::write(&blocked, b"keep").unwrap();
    run(&probe, &blocked, Some("1"), false);
    assert_eq!(fs::read(&blocked).unwrap(), b"keep", "failed recording changed the blocker");
    fs::remove_dir_all(root).unwrap();
}
