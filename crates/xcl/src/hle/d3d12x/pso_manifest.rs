//! `XCL_PSO_MANIFEST=1`: the order in which a run asks for its pipelines, written as it goes
//! to `pso-manifest.txt` beside `used-pipelines.txt` (a new file each run, one tab-separated
//! event a line). Part of #2088 (workstream 1): which pipelines a title needs before each
//! point of its start, in the order it needs them, and which of them it had to wait for.
//!
//! - `use`: a pipeline's first use (its first bind or draw), with its shader-set key, compute
//!   or graphics, and whether it was built by then (`ready`) or not (`cold`).
//! - `build`: a driver compile of a pipeline (`driver_compile`): kind, DXIL bytes, the console
//!   record's hash when known, whether a driver blob was supplied, time in the driver and time
//!   spent waiting for a compile slot.
//! - `present`: the title's presents at milestones (the 1st, 2nd, 4th … 1024th, then every
//!   1024th), so a manifest shows where presents stopped and whether they came back.
//!
//! The `ms` column is the time since the first event. Recording only: nothing is built,
//! ordered or skipped differently.

use std::io::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// One manifest event.
#[derive(Clone, Copy, Debug)]
pub enum Event<'a> {
    Use { key: u64, compute: bool, ready: bool },
    Build { kind: &'a str, bytes: usize, record: Option<(u64, bool)>, driver_ms: u128, waited_ms: u128 },
    Present { n: u64 },
}

pub fn enabled() -> bool {
    static E: OnceLock<bool> = OnceLock::new();
    *E.get_or_init(|| std::env::var("XCL_PSO_MANIFEST").is_ok_and(|v| v.trim() == "1"))
}

/// The header line: the columns of each event kind.
pub const HEADER: &str = "# xcl pso manifest v1: seq\tms\tuse\tkey\tcompute|graphics\tready|cold | seq\tms\tbuild\tkind\tdxil_bytes\trecord|-\tblob|cold|-\tdriver_ms\twaited_ms | seq\tms\tpresent\tn";

/// The manifest line of event `seq` at `ms` (without the newline).
pub fn line(seq: u64, ms: u128, e: &Event) -> String {
    match *e {
        Event::Use { key, compute, ready } => {
            format!("{seq}\t{ms}\tuse\t{key:016x}\t{}\t{}", if compute { "compute" } else { "graphics" }, if ready { "ready" } else { "cold" })
        }
        Event::Build { kind, bytes, record, driver_ms, waited_ms } => {
            let (rec, blob) = match record {
                Some((h, supplied)) => (format!("{h:016x}"), if supplied { "blob" } else { "cold" }),
                None => ("-".to_string(), "-"),
            };
            format!("{seq}\t{ms}\tbuild\t{kind}\t{bytes}\t{rec}\t{blob}\t{driver_ms}\t{waited_ms}")
        }
        Event::Present { n } => format!("{seq}\t{ms}\tpresent\t{n}"),
    }
}

/// Whether the `n`th present (1-based) is a milestone: a power of two up to 1024, then every
/// 1024th.
pub fn present_milestone(n: u64) -> bool {
    n != 0 && (n.is_power_of_two() && n <= 1024 || n % 1024 == 0)
}

struct Manifest {
    start: Instant,
    seq: AtomicU64,
    file: Mutex<Option<std::fs::File>>,
}

fn get() -> &'static Manifest {
    static M: OnceLock<Manifest> = OnceLock::new();
    M.get_or_init(|| {
        // (a test writes no file into the user's cache)
        let file = if cfg!(test) {
            None
        } else {
            let path = crate::gpu::recomp::dxc::cache_dir().join("pso-manifest.txt");
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            match std::fs::File::create(&path) {
                Ok(mut f) => {
                    let _ = writeln!(f, "{HEADER}");
                    crate::xlog_info!("D3D12.X: pipeline requests are written to {}", path.display());
                    Some(f)
                }
                Err(e) => {
                    crate::xlog_warn!("D3D12.X: XCL_PSO_MANIFEST: cannot create {}: {e}", path.display());
                    None
                }
            }
        };
        Manifest { start: Instant::now(), seq: AtomicU64::new(0), file: Mutex::new(file) }
    })
}

/// Record `e` (nothing unless `XCL_PSO_MANIFEST=1`).
pub fn note(e: Event) {
    if !enabled() {
        return;
    }
    let m = get();
    let mut f = m.file.lock().unwrap_or_else(|p| p.into_inner());
    // (sequence numbers in file order: taken under the file's lock)
    let seq = m.seq.fetch_add(1, Ordering::Relaxed);
    if let Some(f) = f.as_mut() {
        let _ = writeln!(f, "{}", line(seq, m.start.elapsed().as_millis(), &e));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_lines_have_one_column_layout_per_event() {
        assert_eq!(line(0, 5, &Event::Use { key: 0xAB, compute: true, ready: false }), "0\t5\tuse\t00000000000000ab\tcompute\tcold");
        assert_eq!(line(1, 6, &Event::Use { key: 1, compute: false, ready: true }), "1\t6\tuse\t0000000000000001\tgraphics\tready");
        assert_eq!(
            line(2, 700, &Event::Build { kind: "compute", bytes: 404008, record: Some((0x7b13a0cd15b6edf9, false)), driver_ms: 3266, waited_ms: 37588 }),
            "2\t700\tbuild\tcompute\t404008\t7b13a0cd15b6edf9\tcold\t3266\t37588"
        );
        assert_eq!(line(3, 701, &Event::Build { kind: "graphics", bytes: 10, record: Some((1, true)), driver_ms: 2, waited_ms: 0 }), "3\t701\tbuild\tgraphics\t10\t0000000000000001\tblob\t2\t0");
        assert_eq!(line(4, 702, &Event::Build { kind: "graphics", bytes: 10, record: None, driver_ms: 2, waited_ms: 0 }), "4\t702\tbuild\tgraphics\t10\t-\t-\t2\t0");
        assert_eq!(line(5, 900, &Event::Present { n: 64 }), "5\t900\tpresent\t64");
        // every event kind's line has as many columns as its part of the header names
        let header: Vec<usize> = HEADER.trim_start_matches("# xcl pso manifest v1: ").split(" | ").map(|p| p.split('\t').count()).collect();
        assert_eq!(header, vec![6, 9, 4]);
    }

    #[test]
    fn presents_are_noted_at_milestones() {
        let first: Vec<u64> = (1..=3000).filter(|&n| present_milestone(n)).collect();
        assert_eq!(first, vec![1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048]);
        assert!(!present_milestone(0) && present_milestone(4096) && present_milestone(3072) && !present_milestone(3071));
    }
}
