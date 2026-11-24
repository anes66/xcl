//! End to end without a game executable: a console compute shader in the retail container
//! layout (DXBC → DXIL part with XDAT → XBCH → FSSM, CS stage descriptor) goes through the
//! D3D12.X backend's CreateComputePipelineState hook as a title would hand it over →
//! recompiled → D3D12 pipeline on the adapter → dispatched → memory read back and checked.
//!
//! The program is wave64, as console compute programs are: on an adapter without 64-lane
//! waves (NVIDIA) the backend has to run it as two wave32 halves, or the driver refuses the
//! pipeline. The pipeline counters are checked, so the test cannot pass on a fallback path.

use std::sync::Mutex;

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D12::*;

use xcl::gpu::rdna2::encode;
use xcl::gpu::recomp::runner;
use xcl::hle::d3d12x::{pso_async, real};

static SERIAL: Mutex<()> = Mutex::new(());

fn le(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

/// Console CS container: DXBC with one DXIL part (program kind CS, no bitcode) whose XDAT
/// holds XBCH (zlib) → FSSM with `code` and a CS stage descriptor: COMPUTE_PGM_RSRC2
/// `rsrc2` at +0x38, numthreads at +0x3C.
fn console_cs(code: &[u8], rsrc2: u32, threads: [u32; 3]) -> Vec<u8> {
    let code_at = 0x308usize;
    let mut fssm = vec![0u8; code_at];
    fssm[0..4].copy_from_slice(b"FSSM");
    fssm[4..8].copy_from_slice(&0x101u32.to_le_bytes());
    fssm[0xC0..0xC8].copy_from_slice(&((code_at - 0x10) as u64).to_le_bytes());
    fssm[0xC8..0xD0].copy_from_slice(&(code.len() as u64).to_le_bytes());
    fssm[0x100..0x108].copy_from_slice(&0x0000_00C0_0002_0106u64.to_le_bytes()); // CS, API stage 1
    for (k, x) in [rsrc2, threads[0], threads[1], threads[2]].into_iter().enumerate() {
        fssm[0x138 + 4 * k..0x13C + 4 * k].copy_from_slice(&x.to_le_bytes());
    }
    fssm.extend_from_slice(code);
    let size = (fssm.len() - 0x10) as u32;
    fssm[8..12].copy_from_slice(&size.to_le_bytes());
    let z = miniz_oxide::deflate::compress_to_vec_zlib(&fssm, 6);
    let mut xbch = b"XBCH".to_vec();
    for x in [3, z.len() as u32, 0, fssm.len() as u32, 0] {
        le(&mut xbch, x);
    }
    xbch.extend_from_slice(&z);
    // DXIL part: program header (kind 5 = CS, 6.6), XDAT, its entries, NOBC placeholder
    let table = 0x18 + 12;
    let data_at = table + 2 * 12;
    let mut d = Vec::new();
    for x in [5 << 16 | 0x66, 0, u32::from_le_bytes(*b"DXIL"), 0x106, 0, 4] {
        le(&mut d, x);
    }
    d.extend_from_slice(b"XDAT");
    for x in [2, table as u32, 0x12, data_at as u32, 16, 0x0d, (data_at + 16) as u32, xbch.len() as u32] {
        le(&mut d, x);
    }
    d.extend_from_slice(&[0x5a; 16]);
    d.extend_from_slice(&xbch);
    while d.len() % 4 != 0 {
        d.push(0);
    }
    let nobc = d.len();
    d.extend_from_slice(b"NOBC");
    let dwords = (d.len() / 4) as u32;
    d[4..8].copy_from_slice(&dwords.to_le_bytes());
    d[16..20].copy_from_slice(&((nobc - 8) as u32).to_le_bytes());
    let mut v = vec![0u8; 36];
    v[0..4].copy_from_slice(b"DXBC");
    v[20..24].copy_from_slice(&1u32.to_le_bytes());
    v[28..32].copy_from_slice(&1u32.to_le_bytes());
    v[32..36].copy_from_slice(&36u32.to_le_bytes());
    v.extend_from_slice(b"DXIL");
    le(&mut v, d.len() as u32);
    v.extend_from_slice(&d);
    let total = v.len() as u32;
    v[24..28].copy_from_slice(&total.to_le_bytes());
    v
}

const V: u64 = 256; // VALU source code of v0

#[test]
fn pipeline_journal_cli_compiles_and_resumes() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    if xcl::gpu::recomp::dxc::location().is_err() {
        runner::skip_test("no DXC");
        return;
    }
    if runner::wave64_programs_supported() != Ok(true) {
        runner::skip_test("no wave64");
        return;
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("xcl_pipeline_cli_{}_{stamp}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let journal = dir.join("pipelines.bin");
    let cache = dir.join("dxil");
    let blob = console_cs(&program(), 2 << 1 | 1 << 7 | 1 << 8 | 1 << 11, [16, 8, 1]);
    let shader_hash = xcl::gpu::shader::fnv64(&blob);
    // The journal stores the shader hash, desc size, then a pointer-free desc.
    let mut payload = shader_hash.to_le_bytes().to_vec();
    let desc_size = std::mem::size_of::<D3D12_COMPUTE_PIPELINE_STATE_DESC>();
    le(&mut payload, desc_size as u32);
    payload.resize(payload.len() + desc_size, 0);
    let hash = xcl::gpu::shader::fnv64(&[payload.as_slice(), &[4]].concat());
    let mut bytes = xcl::hle::d3d12x::pso_journal::MAGIC.to_vec();
    for (tag, windows, hash, data) in [(b'S', 0, shader_hash, blob.as_slice()), (b'C', 4, hash, payload.as_slice()), (b'U', 0, hash, &[][..])] {
        bytes.extend_from_slice(&[tag, windows, 0, 0]);
        le(&mut bytes, data.len() as u32);
        bytes.extend_from_slice(&hash.to_le_bytes());
        bytes.extend_from_slice(data);
    }
    std::fs::write(&journal, bytes).unwrap();
    let command = || {
        let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_xcl"));
        cmd.arg("pipelines").arg(&journal).args(["--threads", "1"])
            .env("XCL_DXIL_CACHE", &cache).env("XCL_PIPELINE_THREADS", "1");
        cmd
    };
    let run = |expected: &str, failed: usize| {
        let out = command().output().unwrap();
        let log = format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        println!("{log}");
        assert!(out.status.success(), "{log}");
        assert!(log.contains(expected), "{log}");
        assert!(log.contains(&format!("failed {failed},")), "{log}");
        assert!(!journal.with_extension("trying").exists(), "a clean exit must remove in-flight state");
        log
    };
    assert!(!cache.exists(), "first run must start without an application cache");
    for args in [&["--limit"][..], &["--limit", "bad"][..]] {
        let rejected = command().args(args).output().unwrap();
        assert_eq!(rejected.status.code(), Some(2), "invalid limit must not compile the journal: {rejected:?}");
        assert!(!journal.with_extension("done").exists() && !journal.with_extension("trying").exists());
        assert!(!cache.exists());
    }
    let zero = command().args(["--limit", "0"]).output().unwrap();
    assert!(zero.status.success() && String::from_utf8_lossy(&zero.stdout).contains("made 0, failed 0"), "{zero:?}");
    assert_eq!(std::fs::metadata(journal.with_extension("done")).unwrap().len(), 8, "zero attempts: only the binary key");
    assert!(!cache.exists() && !journal.with_extension("trying").exists());
    std::fs::remove_file(journal.with_extension("done")).unwrap();
    run("made 1,", 0);
    let done = journal.with_extension("done");
    let progress = std::fs::read(&done).unwrap();
    assert_eq!(progress.len(), 16, "binary key and one completed pipeline");
    assert_eq!(&progress[8..], &hash.to_le_bytes());
    // The second process must refuse a progress file held by another writer.
    use std::os::windows::fs::OpenOptionsExt;
    let held = std::fs::OpenOptions::new().read(true).write(true).share_mode(1).open(&done).unwrap();
    let refused = command().output().unwrap();
    assert_eq!(refused.status.code(), Some(2), "{refused:?}");
    assert!(String::from_utf8_lossy(&refused.stderr).contains("pipelines.done"), "{refused:?}");
    assert_eq!(std::fs::read(&done).unwrap(), progress);
    assert!(!journal.with_extension("trying").exists());
    drop(held);
    // A torn final progress record must not offset the next completed hash.
    std::fs::write(&done, [progress.as_slice(), &[0xaa, 0xbb, 0xcc]].concat()).unwrap();
    let resumed = run("made 0,", 0);
    assert!(resumed.contains("1 done before"), "{resumed}");
    assert_eq!(std::fs::read(&done).unwrap(), progress);
    // A different binary key invalidates progress, even with a populated DXIL cache.
    let mut stale = progress.clone();
    stale[0] ^= 1;
    std::fs::write(&done, stale).unwrap();
    run("made 1,", 0);
    assert_eq!(std::fs::read(&done).unwrap(), progress);
    // A new binary/driver key must not accept an old blob merely because its file exists.
    let dxil_file = std::fs::read_dir(&cache).unwrap().flatten().map(|e| e.path())
        .find(|p| p.extension().is_some_and(|ext| ext == "dxil")).unwrap();
    let dxil = std::fs::read(dxil_file).unwrap();
    let driver_file = cache.join("driver").join(format!("{:016x}-{:x}-0.pso", xcl::gpu::shader::fnv64(&dxil), dxil.len()));
    std::fs::create_dir_all(driver_file.parent().unwrap()).unwrap();
    let invalid = b"invalid cached journal pipeline";
    std::fs::write(&driver_file, invalid).unwrap();
    std::fs::remove_file(&done).unwrap();
    run("made 1,", 0);
    assert_ne!(std::fs::read(&driver_file).ok().as_deref(), Some(invalid.as_slice()), "an invalid blob must be dropped or replaced");
    assert_eq!(std::fs::read(&done).unwrap(), progress);
    // Seed a valid blob through the existing offline compiler and require actual reuse.
    let input = dir.join("input");
    std::fs::create_dir(&input).unwrap();
    std::fs::write(input.join("kernel.bin"), &blob).unwrap();
    let seeded = std::process::Command::new(env!("CARGO_BIN_EXE_xcl"))
        .arg("shaders").arg(&input).args(["--precompile", "--pipelines"])
        .env("XCL_DXIL_CACHE", &cache).env("XCL_PIPELINE_THREADS", "1").output().unwrap();
    assert!(seeded.status.success(), "{seeded:?}");
    let valid = std::fs::read(&driver_file).unwrap();
    assert!(!valid.is_empty());
    std::fs::remove_file(&done).unwrap();
    let reused = run("made 1,", 0);
    assert!(reused.contains("compute pipelines made from the driver's blob"), "{reused}");
    assert_eq!(std::fs::read(&driver_file).unwrap(), valid);
    // An incomplete compute payload cannot be replayed and must remain retryable.
    let mut broken = vec![b'C', 4, 0, 0];
    le(&mut broken, 8);
    broken.extend_from_slice(&(hash ^ 1).to_le_bytes());
    broken.extend_from_slice(&shader_hash.to_le_bytes());
    use std::io::Write;
    std::fs::OpenOptions::new().append(true).open(&journal).unwrap().write_all(&broken).unwrap();
    for _ in 0..2 {
        let retried = run("made 0,", 1);
        assert!(retried.contains("1 pipelines to make"), "{retried}");
        assert!(retried.contains("not readable from the journal"), "{retried}");
        assert_eq!(std::fs::read(&done).unwrap(), progress, "a failure must not enter completed progress");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn offline_compute_pipeline_is_reused_by_a_second_process() {
    use std::{fs, process::Command};
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    if xcl::gpu::recomp::dxc::location().is_err() {
        runner::skip_test("no DXC");
        return;
    }
    if runner::wave64_programs_supported() != Ok(true) {
        runner::skip_test("no GPU with 32- or 64-lane waves");
        return;
    }
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let dir = std::env::temp_dir().join(format!("xcl-offline-pso-{}-{nonce}", std::process::id()));
    let input = dir.join("input");
    let cache = dir.join("cache");
    fs::create_dir_all(&input).unwrap();
    fs::write(input.join("kernel.bin"), console_cs(&program(), 2 << 1 | 1 << 7 | 1 << 8 | 1 << 11, [16, 8, 1])).unwrap();
    let run = |enabled: bool| {
        let out = Command::new(env!("CARGO_BIN_EXE_xcl")).arg("shaders").arg(&input)
            .args(["--precompile", "--pipelines"]).env("XCL_DXIL_CACHE", &cache)
            .env("XCL_PSO_DISK_CACHE", if enabled { "1" } else { "0" }).output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(out.status.success() && text.contains("pipelines made on the GPU 1, refused 0"), "{text}\n{}", String::from_utf8_lossy(&out.stderr));
        text
    };
    run(false);
    assert!(!cache.join("driver").exists());
    run(true);
    let dxil_file = fs::read_dir(&cache).unwrap().flatten().map(|e| e.path())
        .find(|p| p.extension().is_some_and(|ext| ext == "dxil")).unwrap();
    let dxil = fs::read(dxil_file).unwrap();
    let name = format!("{:016x}-{:x}-0.pso", xcl::gpu::shader::fnv64(&dxil), dxil.len());
    let driver_file = cache.join("driver").join(name);
    assert!(driver_file.metadata().unwrap().len() > 0);
    let warm = run(true);
    // A blob from an earlier driver can be refused. The normal build must still succeed.
    fs::write(&driver_file, b"invalid cached pipeline").unwrap();
    run(true);
    let _ = fs::remove_dir_all(&dir);
    assert!(warm.contains("pipelines loaded from the driver cache 1"), "{warm}");
}

/// 16×8 threads per group (two wave64s), s[0:1] = output base and row width, TGID_X/Y in
/// s2/s3, thread ids in v0/v1: each thread writes its global (x, y), and through LDS +
/// s_barrier the (x, y) of the thread at the mirrored index, which is in the other wave.
fn program() -> Vec<u8> {
    let list: &[(&str, &str, &[(&str, u64)], Option<u32>)] = &[
        ("V_LSHLREV_B32", "ENC_VOP2", &[("VDST", 2), ("SRC0", 132), ("VSRC1", 1)], None), // y · 16
        ("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 2), ("SRC0", V + 2), ("VSRC1", 0)], None), // index in group
        ("S_LSHL_B32", "ENC_SOP2", &[("SDST", 4), ("SSRC0", 2), ("SSRC1", 132)], None), // group x · 16
        ("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 3), ("SRC0", 4), ("VSRC1", 0)], None), // global x
        ("S_LSHL_B32", "ENC_SOP2", &[("SDST", 5), ("SSRC0", 3), ("SSRC1", 131)], None), // group y · 8
        ("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 4), ("SRC0", 5), ("VSRC1", 1)], None), // global y
        ("V_LSHLREV_B32", "ENC_VOP2", &[("VDST", 5), ("SRC0", 144), ("VSRC1", 4)], None),
        ("V_OR_B32", "ENC_VOP2", &[("VDST", 5), ("SRC0", V + 5), ("VSRC1", 3)], None), // x | y << 16
        ("V_LSHLREV_B32", "ENC_VOP2", &[("VDST", 6), ("SRC0", 130), ("VSRC1", 2)], None),
        ("DS_WRITE_B32", "ENC_DS", &[("ADDR", 6), ("DATA0", 5)], None),
        ("S_BARRIER", "ENC_SOPP", &[], None),
        ("V_SUB_NC_U32", "ENC_VOP2", &[("VDST", 7), ("SRC0", 255), ("VSRC1", 2)], Some(127)), // mirrored thread
        ("V_LSHLREV_B32", "ENC_VOP2", &[("VDST", 7), ("SRC0", 130), ("VSRC1", 7)], None),
        ("DS_READ_B32", "ENC_DS", &[("VDST", 12), ("ADDR", 7)], None),
        ("V_MOV_B32", "ENC_VOP1", &[("VDST", 11), ("SRC0", V + 5)], None),
        ("V_MUL_LO_U32", "ENC_VOP3", &[("VDST", 9), ("SRC0", V + 4), ("SRC1", 1)], None), // y · width
        ("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 9), ("SRC0", V + 9), ("VSRC1", 3)], None),
        ("V_LSHLREV_B32", "ENC_VOP2", &[("VDST", 9), ("SRC0", 131), ("VSRC1", 9)], None),
        ("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 9), ("SRC0", 0), ("VSRC1", 9)], None), // + output base
        ("V_MOV_B32", "ENC_VOP1", &[("VDST", 10), ("SRC0", 128)], None),
        ("GLOBAL_STORE_DWORDX2", "ENC_FLAT_GLBL", &[("ADDR", 9), ("DATA", 11), ("SADDR", 0x7D)], None),
        ("S_ENDPGM", "ENC_SOPP", &[], None),
    ];
    list.iter().flat_map(|(n, e, f, l)| encode(n, e, f, *l).unwrap_or_else(|x| panic!("{n}: {x}"))).collect()
}

#[test]
fn console_wave64_compute_through_the_pso_hook_dispatch_and_read_back() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    if xcl::gpu::recomp::dxc::location().is_err() {
        println!("no DXC – skipping");
        runner::skip_test("no DXC");
        return;
    }
    if xcl::hle::d3d12x::backend() != xcl::hle::d3d12x::Backend::Real {
        println!("no real D3D12 device – skipping");
        runner::skip_test("no GPU");
        return;
    }
    // 32- or 64-lane waves (not WARP / the Basic Render Driver: 4): the driver rejects the
    // translated program otherwise
    if runner::wave64_programs_supported() != Ok(true) {
        println!("no 32- or 64-lane waves – skipping");
        runner::skip_test("no wave64");
        return;
    }
    let raw = real::device_raw();
    let dev = unsafe { ID3D12Device::from_raw_borrowed(&raw) }.unwrap().clone();
    println!("backend device: {}", real::stats().adapter);

    // user SGPRs 2, TGID_X + TGID_Y, thread ids v0 + v1
    let blob = console_cs(&program(), 2 << 1 | 1 << 7 | 1 << 8 | 1 << 11, [16, 8, 1]);
    assert!(xcl::gpu::xbox::is_xbox(&blob), "expected a console container");
    let game_rs: ID3D12RootSignature = unsafe {
        let mut ser = None;
        D3D12SerializeRootSignature(&D3D12_ROOT_SIGNATURE_DESC::default(), D3D_ROOT_SIGNATURE_VERSION_1, &mut ser, None).unwrap();
        let ser = ser.unwrap();
        dev.CreateRootSignature(0, std::slice::from_raw_parts(ser.GetBufferPointer() as *const u8, ser.GetBufferSize())).unwrap()
    };
    let mut desc = D3D12_COMPUTE_PIPELINE_STATE_DESC {
        pRootSignature: std::mem::ManuallyDrop::new(Some(game_rs)),
        CS: D3D12_SHADER_BYTECODE { pShaderBytecode: blob.as_ptr() as _, BytecodeLength: blob.len() },
        ..Default::default()
    };
    let before = real::stats();
    let pso: Result<ID3D12PipelineState, _> = unsafe { dev.CreateComputePipelineState(&desc) };
    unsafe { std::mem::ManuallyDrop::drop(&mut desc.pRootSignature) };
    let pso = pso.expect("CreateComputePipelineState through the backend hook");
    // built in the background behind a stand-in when its shader was not compiled yet
    let real_raw = xcl::hle::d3d12x::pso_async::resolve_wait(pso.as_raw() as usize) as *mut std::ffi::c_void;
    let after = real::stats();
    assert_eq!(after.translated_pipelines, before.translated_pipelines + 1, "PSO was not built from the translated shader: {:?}", after.translation_errors);
    assert!(
        real::is_translated_compute(real_raw as usize),
        "resolved compute PSO must be registered in TRANSLATED_COMPUTE"
    );
    let pso = unsafe { ID3D12PipelineState::from_raw_borrowed(&real_raw) }.unwrap().clone();
    let rs_raw = real::pso_root_signature(pso.as_raw() as usize).expect("root signature of the translated PSO") as *mut std::ffi::c_void;
    let rs = unsafe { ID3D12RootSignature::from_raw_borrowed(&rs_raw) }.unwrap().clone();

    let (w, h, base) = (32usize, 24usize, 0x100usize);
    let mut io = vec![0u32; 128];
    io[0] = base as u32;
    io[1] = w as u32;
    let out = runner::dispatch_on(&dev, &pso, &rs, [2, 3, 1], &io, &vec![0u32; (base + w * h * 8) / 4]).expect("dispatch");
    for y in 0..h {
        for x in 0..w {
            let at = (base + (y * w + x) * 8) / 4;
            assert_eq!(out[at], (x | y << 16) as u32, "({x}, {y}): own value");
            let (gx, gy, i) = (x / 16, y / 8, (y % 8) * 16 + x % 16);
            let (px, py) = (gx * 16 + (127 - i) % 16, gy * 8 + (127 - i) / 16);
            assert_eq!(out[at + 1], (px | py << 16) as u32, "({x}, {y}): value of thread {} through LDS", 127 - i);
        }
    }
}

/// Cold DXIL cache: `CreateComputePipelineState` returns a stand-in immediately and compiles
/// the real compute PSO on the background worker. The real PSO (and NOT the stand-in) must be
/// registered in `TRANSLATED_COMPUTE` once the background build completes.
#[test]
fn async_compute_pso_registers_real_pso_and_not_standin() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    if xcl::gpu::recomp::dxc::location().is_err() {
        println!("no DXC – skipping");
        runner::skip_test("no DXC");
        return;
    }
    if xcl::hle::d3d12x::backend() != xcl::hle::d3d12x::Backend::Real {
        println!("no real D3D12 device – skipping");
        runner::skip_test("no GPU");
        return;
    }
    // 32- or 64-lane waves (not WARP / the Basic Render Driver: 4): the driver rejects the
    // translated program otherwise
    if runner::wave64_programs_supported() != Ok(true) {
        println!("no 32- or 64-lane waves – skipping");
        runner::skip_test("no wave64");
        return;
    }
    if !pso_async::enabled() {
        println!("XCL_ASYNC_PSO=0 – skipping");
        return;
    }

    struct ColdCache(std::path::PathBuf, Option<std::ffi::OsString>);
    impl Drop for ColdCache {
        fn drop(&mut self) {
            match &self.1 {
                Some(prev) => std::env::set_var("XCL_DXIL_CACHE", prev),
                None => std::env::remove_var("XCL_DXIL_CACHE"),
            }
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let cold = ColdCache(
        std::env::temp_dir().join(format!("xcl-async-compute-pso-{}", std::process::id())),
        std::env::var_os("XCL_DXIL_CACHE"),
    );
    let _ = std::fs::remove_dir_all(&cold.0);
    std::env::set_var("XCL_DXIL_CACHE", &cold.0);

    let raw = real::device_raw();
    let dev = unsafe { ID3D12Device::from_raw_borrowed(&raw) }.unwrap().clone();

    // Distinct minimal console CS so neither disk nor in-memory DXIL cache already holds it.
    let code: Vec<u8> = [
        ("V_MOV_B32", "ENC_VOP1", &[("VDST", 2), ("SRC0", 0)][..], None),
        ("S_ENDPGM", "ENC_SOPP", &[][..], None),
    ]
    .iter()
    .flat_map(|(n, e, f, l)| encode(n, e, f, *l).unwrap_or_else(|x| panic!("{n}: {x}")))
    .collect();
    let blob = console_cs(&code, 2 << 1 | 1 << 7, [8, 8, 1]);

    let game_rs: ID3D12RootSignature = unsafe {
        let mut ser = None;
        D3D12SerializeRootSignature(&D3D12_ROOT_SIGNATURE_DESC::default(), D3D_ROOT_SIGNATURE_VERSION_1, &mut ser, None).unwrap();
        let ser = ser.unwrap();
        dev.CreateRootSignature(0, std::slice::from_raw_parts(ser.GetBufferPointer() as *const u8, ser.GetBufferSize())).unwrap()
    };
    let mut desc = D3D12_COMPUTE_PIPELINE_STATE_DESC {
        pRootSignature: std::mem::ManuallyDrop::new(Some(game_rs)),
        CS: D3D12_SHADER_BYTECODE { pShaderBytecode: blob.as_ptr() as _, BytecodeLength: blob.len() },
        ..Default::default()
    };
    let pso: Result<ID3D12PipelineState, _> = unsafe { dev.CreateComputePipelineState(&desc) };
    unsafe { std::mem::ManuallyDrop::drop(&mut desc.pRootSignature) };
    let pso = pso.expect("CreateComputePipelineState stand-in");
    let standin_raw = pso.as_raw() as usize;

    assert!(!real::is_translated_compute(standin_raw), "stand-in compute PSO must not be registered in TRANSLATED_COMPUTE");
    let real_raw = pso_async::resolve_wait(standin_raw);
    assert_ne!(real_raw, standin_raw, "cold DXIL cache must build compute PSO asynchronously behind a stand-in");
    assert!(real::is_translated_compute(real_raw), "background-built real compute PSO must be registered in TRANSLATED_COMPUTE");
    assert!(!real::is_translated_compute(standin_raw), "stand-in must remain unregistered after background build");
    assert!(real::pso_root_signature(real_raw).is_some(), "real compute PSO must have an ABI root signature");

    // Releasing the stand-in must release and forget the background-built real PSO too.
    drop(pso);
    assert!(!real::is_translated_compute(real_raw), "forgetting released async compute PSO must clean up TRANSLATED_COMPUTE");
}
