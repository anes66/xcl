//! The compact form (`compact`) against the plain one.

use super::kernel::{compact_mode, translate_full, translate_full_as, KernelOptions};
use crate::gpu::rdna2::encode;
use crate::gpu::recomp::runner::{self, KernelState};
use crate::gpu::recomp::KERNEL_LANES;

fn repeated(copies: usize) -> Vec<u8> {
    let i = |n: &str, e: &str, f: &[(&str, u64)]| encode(n, e, f, None).unwrap_or_else(|x| panic!("{n}: {x}"));
    let mut v = Vec::new();
    for _ in 0..copies {
        v.extend(i("S_ADD_U32", "ENC_SOP2", &[("SDST", 0), ("SSRC0", 0), ("SSRC1", 1)]));
        v.extend(i("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 1), ("SRC0", 256), ("VSRC1", 1)]));
        v.extend(i("S_ADD_U32", "ENC_SOP2", &[("SDST", 5), ("SSRC0", 5), ("SSRC1", 130)]));
        v.extend(i("V_CMP_LT_U32", "ENC_VOPC", &[("SRC0", 5), ("VSRC1", 0)]));
        v.extend(i("S_AND_SAVEEXEC_B64", "ENC_SOP1", &[("SDST", 6), ("SSRC0", 106)]));
        v.extend(i("S_CBRANCH_EXECZ", "ENC_SOPP", &[("SIMM16", 1)]));
        v.extend(i("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 3), ("SRC0", 256), ("VSRC1", 3)]));
        v.extend(i("S_MOV_B64", "ENC_SOP1", &[("SDST", 126), ("SSRC0", 6)]));
        v.extend(i("S_CMP_LG_U32", "ENC_SOPC", &[("SSRC0", 0), ("SSRC1", 2)]));
        v.extend(i("S_CBRANCH_SCC1", "ENC_SOPP", &[("SIMM16", 1)]));
        v.extend(i("S_ADD_U32", "ENC_SOP2", &[("SDST", 3), ("SSRC0", 3), ("SSRC1", 129)]));
    }
    v.extend(i("S_MOV_B32", "ENC_SOP1", &[("SDST", 4), ("SSRC0", 133)]));
    v.extend(i("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 2), ("SRC0", 256), ("VSRC1", 2)]));
    v.extend(i("S_SUB_U32", "ENC_SOP2", &[("SDST", 4), ("SSRC0", 4), ("SSRC1", 129)]));
    v.extend(i("S_CMP_LG_U32", "ENC_SOPC", &[("SSRC0", 4), ("SSRC1", 128)]));
    v.extend(i("S_CBRANCH_SCC1", "ENC_SOPP", &[("SIMM16", 0xFFFC)]));
    v.extend(i("S_ENDPGM", "ENC_SOPP", &[]));
    v
}

fn size(copies: usize, o: KernelOptions, compact: bool) -> usize {
    translate_full_as(&repeated(copies), o, compact).unwrap_or_else(|e| panic!("{copies} copies, compact {compact}: {e}")).dxil.len()
}

#[test]
fn the_compact_form_lowers_a_repeated_block_once() {
    for (wave64, split) in [(false, false), (true, true), (true, false)] {
        let o = KernelOptions { wave64, split, mem_windows: 1, sampler_base: 0 };
        let (compact, plain) = (size(128, o, true) - size(64, o, true), size(128, o, false) - size(64, o, false));
        assert!(compact * 20 < plain, "wave64 {wave64} split {split}: 64 more copies cost {compact} B compact, {plain} B plain");
    }
}

#[test]
fn a_large_program_that_repeats_itself_takes_the_compact_form() {
    if compact_mode().is_some() {
        return;
    }
    let o = KernelOptions { wave64: true, split: true, mem_windows: 1, sampler_base: 0 };
    for (copies, compact) in [(2500, true), (2000, false)] {
        assert_eq!(translate_full(&repeated(copies), o).unwrap().dxil.len(), size(copies, o, compact), "{copies} copies");
    }
}

#[test]
fn gpu_compact_form_runs_as_the_plain_form() {
    match (runner::adapter(), runner::wave64_programs_supported(), crate::gpu::recomp::dxc::location()) {
        (Ok(_), Ok(true), Ok(_)) => {}
        (a, _, d) => return runner::skip_test(if a.is_err() { "no GPU" } else if d.is_err() { "no DXC" } else { "no wave64" }),
    }
    let mut st = KernelState::default();
    (st.sgpr[1], st.sgpr[2]) = (3, 12);
    for lane in 0..KERNEL_LANES {
        st.vgpr[lane][0] = lane as u32;
    }
    let code = repeated(40);
    let o = KernelOptions { wave64: true, split: runner::split_wave64(), mem_windows: 1, sampler_base: 0 };
    let run = |compact: bool| {
        let k = translate_full_as(&code, o, compact).unwrap_or_else(|e| panic!("compact {compact}: {e}"));
        runner::run_with_statics(&k.dxil, &st, &[], &[], &[], &k.static_samplers).expect("GPU run")
    };
    let (c, p) = (run(true), run(false));
    assert_eq!(&c.sgpr[..6], &[120, 3, 12, 1, 0, 80]);
    for lane in 0..KERNEL_LANES as u32 {
        let masked = lane * (lane.saturating_sub(1) / 2);
        assert_eq!(&c.vgpr[lane as usize][..4], &[lane, 40 * lane, 5 * lane, masked], "lane {lane}");
    }
    assert_eq!((c.sgpr, c.vgpr, c.scc), (p.sgpr, p.vgpr, p.scc));
}

#[test]
fn gpu_compact_form_spends_the_loop_budget_as_the_plain_form() {
    match (runner::adapter(), runner::wave64_programs_supported(), crate::gpu::recomp::dxc::location()) {
        (Ok(_), Ok(true), Ok(_)) => {}
        (a, _, d) => return runner::skip_test(if a.is_err() { "no GPU" } else if d.is_err() { "no DXC" } else { "no wave64" }),
    }
    let mut v = Vec::new();
    v.extend(encode("S_ADD_U32", "ENC_SOP2", &[("SDST", 0), ("SSRC0", 0), ("SSRC1", 129)], None).unwrap());
    v.extend(encode("S_CMP_LG_U32", "ENC_SOPC", &[("SSRC0", 0), ("SSRC1", 255)], Some(super::lower::LOOP_BUDGET + 1)).unwrap());
    v.extend(encode("S_CBRANCH_SCC1", "ENC_SOPP", &[("SIMM16", 0xFFFC)], None).unwrap());
    v.extend(encode("S_MOV_B32", "ENC_SOP1", &[("SDST", 1), ("SSRC0", 129)], None).unwrap());
    v.extend(encode("S_ENDPGM", "ENC_SOPP", &[], None).unwrap());
    let o = KernelOptions { wave64: true, split: runner::split_wave64(), mem_windows: 1, sampler_base: 0 };
    let run = |compact: bool| {
        let k = translate_full_as(&v, o, compact).unwrap_or_else(|e| panic!("compact {compact}: {e}"));
        runner::run_with_statics(&k.dxil, &KernelState::default(), &[], &[], &[], &k.static_samplers).expect("GPU run")
    };
    let (c, p) = (run(true), run(false));
    assert_eq!(&c.sgpr[..2], &[super::lower::LOOP_BUDGET, 0]);
    assert_eq!(&c.sgpr[..2], &p.sgpr[..2]);
}

fn early_outs(copies: usize) -> Vec<u8> {
    let i = |n: &str, e: &str, f: &[(&str, u64)]| encode(n, e, f, None).unwrap_or_else(|x| panic!("{n}: {x}"));
    let mut v = Vec::new();
    for k in 0..copies {
        v.extend(i("S_ADD_U32", "ENC_SOP2", &[("SDST", 0), ("SSRC0", 0), ("SSRC1", 1)]));
        v.extend(i("S_CMP_EQ_U32", "ENC_SOPC", &[("SSRC0", 0), ("SSRC1", 2)]));
        v.extend(i("S_CBRANCH_SCC1", "ENC_SOPP", &[("SIMM16", ((copies - 1 - k) * 3 + 1) as u64)]));
    }
    v.extend(i("S_MOV_B32", "ENC_SOP1", &[("SDST", 3), ("SSRC0", 129)]));
    v.extend(i("S_ENDPGM", "ENC_SOPP", &[]));
    v
}

#[test]
fn branches_to_many_places_keep_their_target_in_the_entry() {
    let code = early_outs(1000);
    let cfg = super::cfg::Cfg::build(&code).unwrap();
    let leaders = cfg.blocks.iter().filter_map(|b| b.insts.first().map(|&i| cfg.insts[i].pc)).collect();
    let o = KernelOptions { wave64: true, split: true, mem_windows: 1, sampler_base: 0 };
    let lw = super::lower::Lw::new(&cfg.insts, &leaders, o, &super::kernel::Entry::Kernel);
    let p = super::compact::plan(&cfg, &lw).unwrap();
    assert_eq!((p.order.len(), p.steps.len()), (1002, 1002));
    assert!(p.words() < 500, "{} words", p.words());
}

fn compact_table_words(bc: &[u8], name: &str) -> Option<Vec<u32>> {
    let root = super::bitreader_tests::parse(bc);
    let module = root.find(8).unwrap();
    let globals = module.records_with(7);
    let functions = module.records_with(8);
    let symbols = module.find(14).unwrap();
    let symbol = symbols.records_with(1).into_iter().find(|r| r[1..].iter().copied().eq(name.bytes().map(u64::from)))?;
    let initializer = globals[symbol[0] as usize][2] as usize - 1;
    let base = globals.len() + functions.len();
    let constants: Vec<_> = module.find(11).unwrap().records.iter().filter(|r| r.0 != 1).collect();
    let aggregate = constants[initializer - base];
    assert_eq!(aggregate.0, 7, "table initializer must be an aggregate");
    Some(aggregate.1.iter().map(|&id| {
        let integer = constants[id as usize - base];
        assert_eq!(integer.0, 4, "table elements must be integers");
        let encoded = integer.1[0];
        let magnitude = (encoded >> 1) as i64;
        (if encoded & 1 == 0 { magnitude } else { -magnitude }) as u32
    }).collect())
}

#[test]
fn compact_packed_tables_preserve_every_class_and_successor() {
    use super::cfg::Term;
    use super::module::Block;
    let mut checked = 0usize;
    let mut crossed = 0usize;
    for copies in [1, 2, 3, 7, 8, 15, 16, 31, 32, 63, 64, 127, 128, 255, 256, 511, 512, 1000] {
        for with_loop in [false, true] {
            let code = if with_loop { repeated(copies) } else { early_outs(copies) };
            for inline in [false, true] {
                let cfg = super::cfg::Cfg::build(&code).unwrap();
                let leaders = cfg.blocks.iter().filter_map(|b| b.insts.first().map(|&i| cfg.insts[i].pc)).collect();
                let o = KernelOptions { wave64: true, split: true, mem_windows: 1, sampler_base: 0 };
                let mut lw = super::lower::Lw::new(&cfg.insts, &leaders, o, &super::kernel::Entry::Kernel);
                lw.setup_lanes();
                let lane = lw.lane;
                let stride = lw.c((crate::gpu::recomp::KERNEL_VGPRS * 4) as u32);
                let base = lw.mul(lane, stride);
                lw.init.vgpr_base = Some(lw.addc(base, (crate::gpu::recomp::KERNEL_SGPRS * 4) as u32));
                let mut p = super::compact::plan(&cfg, &lw).unwrap();
                let index = |b: usize| p.order.iter().position(|&x| x == b).unwrap() as u32;
                for (n, &b) in p.order.iter().enumerate() {
                    if let Term::Cond(_, _, f) = cfg.blocks[b].term {
                        assert_eq!(index(f), n as u32 + 1, "fixture must support inline targets");
                    }
                }
                // Exercise both encodings on the same CFG, including backward offsets.
                p.inline = inline;
                let width = |n: usize| (1..=32).find(|&b| (n.saturating_sub(1) as u64) < (1u64 << b)).unwrap();
                let cb = width(p.rep.len());
                let pb = width(if inline { p.order.len() } else { p.steps.len() });
                let bits = cb + pb;
                let head = lw.block(None);
                let exit = lw.block(None);
                lw.edge(Block(0), head);
                super::compact::emit(&mut lw, &cfg, &p, head, exit).unwrap();
                lw.s.m.set_block(exit);
                lw.seal(exit);
                lw.s.m.ret();
                lw.s.m.set_block(Block(0));
                lw.s.m.br(head);
                let bc = lw.s.m.to_bitcode("dxil-ms-dx", "");
                let packed = compact_table_words(&bc, "xcl_blocks").unwrap();
                let steps = compact_table_words(&bc, "xcl_steps");
                assert_eq!(steps.is_none(), inline);
                assert_eq!(packed.len() + steps.as_ref().map_or(0, Vec::len), p.words());
                assert_eq!(*packed.last().unwrap(), 0, "the extra read word must be zero");
                for (n, &b) in p.order.iter().enumerate() {
                    // Decode one bit at a time, independently of the emitter's cross-word shifts.
                    let mut entry = 0u64;
                    for bit in 0..bits as usize {
                        let at = n * bits as usize + bit;
                        entry |= (((packed[at / 32] >> (at % 32)) & 1) as u64) << bit;
                    }
                    crossed += usize::from(n * bits as usize % 32 + bits as usize > 32);
                    checked += 1;
                    let class = (entry & ((1u64 << cb) - 1)) as u32;
                    let field = (entry >> cb) as u32;
                    assert_eq!(class, p.class[n], "copies={copies}, loop={with_loop}, inline={inline}, block={n}");
                    let successors = if let Some(steps) = &steps {
                        ((n as u32).wrapping_add(steps[field as usize * 2]),
                            (n as u32).wrapping_add(steps[field as usize * 2 + 1]))
                    } else { (field, n as u32 + 1) };
                    let index = |target| p.order.iter().position(|&x| x == target).unwrap() as u32;
                    match cfg.blocks[b].term {
                        Term::Jump(t) => assert_eq!(successors.0, index(t)),
                        Term::Cond(_, t, f) => assert_eq!(successors, (index(t), index(f))),
                        Term::End if inline => assert_eq!(field, 0),
                        Term::End => assert_eq!(successors, (n as u32, n as u32)),
                    }
                }
            }
        }
    }
    assert!(checked > 10_000 && crossed > 1_000, "checked={checked}, crossed={crossed}");
    println!("packed table entries checked: {checked}; cross-word entries: {crossed}");
}

#[test]
fn compact_inline_targets_require_the_actual_conditional_fallthrough() {
    use super::cfg::Term;
    let mut cfg = super::cfg::Cfg::build(&early_outs(64)).unwrap();
    let leaders = cfg.blocks.iter().filter_map(|b| b.insts.first().map(|&i| cfg.insts[i].pc)).collect();
    let o = KernelOptions { wave64: true, split: true, mem_windows: 1, sampler_base: 0 };
    let lw = super::lower::Lw::new(&cfg.insts, &leaders, o, &super::kernel::Entry::Kernel);
    assert!(super::compact::plan(&cfg, &lw).unwrap().inline);
    let Term::Cond(_, t, f) = &mut cfg.blocks[0].term else { panic!("conditional fixture") };
    std::mem::swap(t, f);
    let p = super::compact::plan(&cfg, &lw).unwrap();
    assert!(!p.inline, "a conditional false edge cannot be replaced by the next block");
    let Term::Cond(_, t, f) = cfg.blocks[0].term else { unreachable!() };
    assert_eq!(p.next[0], (t as u32, f as u32));
}

#[test]
fn gpu_compact_form_with_targets_in_the_entry_runs_as_the_plain_form() {
    match (runner::adapter(), runner::wave64_programs_supported(), crate::gpu::recomp::dxc::location()) {
        (Ok(_), Ok(true), Ok(_)) => {}
        (a, _, d) => return runner::skip_test(if a.is_err() { "no GPU" } else if d.is_err() { "no DXC" } else { "no wave64" }),
    }
    let mut st = KernelState::default();
    (st.sgpr[1], st.sgpr[2]) = (3, 12);
    let code = early_outs(40);
    let o = KernelOptions { wave64: true, split: runner::split_wave64(), mem_windows: 1, sampler_base: 0 };
    let run = |compact: bool| {
        let k = translate_full_as(&code, o, compact).unwrap_or_else(|e| panic!("compact {compact}: {e}"));
        runner::run_with_statics(&k.dxil, &st, &[], &[], &[], &k.static_samplers).expect("GPU run")
    };
    let (c, p) = (run(true), run(false));
    assert_eq!(&c.sgpr[..4], &[12, 3, 12, 0]);
    assert_eq!((c.sgpr, c.vgpr, c.scc), (p.sgpr, p.vgpr, p.scc));
}

#[test]
fn a_mask_read_whole_after_one_copy_only_splits_the_class() {
    let i = |n: &str, e: &str, f: &[(&str, u64)]| encode(n, e, f, None).unwrap_or_else(|x| panic!("{n}: {x}"));
    let cmp = |v: &mut Vec<u8>| {
        v.extend(i("V_CMP_GT_U32", "ENC_VOPC", &[("SRC0", 160), ("VSRC1", 0)]));
        v.extend(i("S_CBRANCH_SCC1", "ENC_SOPP", &[("SIMM16", 0)]));
    };
    let mut v = Vec::new();
    cmp(&mut v);
    v.extend(i("V_CNDMASK_B32", "ENC_VOP2", &[("VDST", 2), ("SRC0", 128), ("VSRC1", 3)]));
    v.extend(i("S_CBRANCH_SCC1", "ENC_SOPP", &[("SIMM16", 0)]));
    cmp(&mut v);
    v.extend(i("S_MOV_B32", "ENC_SOP1", &[("SDST", 4), ("SSRC0", 106)]));
    v.extend(i("S_ENDPGM", "ENC_SOPP", &[]));
    let cfg = super::cfg::Cfg::build(&v).unwrap();
    let leaders = cfg.blocks.iter().filter_map(|b| b.insts.first().map(|&i| cfg.insts[i].pc)).collect();
    let o = KernelOptions { wave64: true, split: true, mem_windows: 1, sampler_base: 0 };
    let lw = super::lower::Lw::new(&cfg.insts, &leaders, o, &super::kernel::Entry::Kernel);
    let p = super::compact::plan(&cfg, &lw).unwrap();
    assert_eq!(p.order.len(), 4);
    assert_ne!(p.class[0], p.class[2]);
}

fn two_segments(copies: usize) -> Vec<u8> {
    let i = |n: &str, e: &str, f: &[(&str, u64)]| encode(n, e, f, None).unwrap_or_else(|x| panic!("{n}: {x}"));
    let mut code = Vec::new();
    code.extend(i("S_MOV_B32", "ENC_SOP1", &[("SDST", 2), ("SSRC0", 2)]));
    for r in [2, 3] {
        code.extend(i("V_MOV_B32", "ENC_VOP1", &[("VDST", r), ("SRC0", 256 + r)]));
    }
    for marker in [3, 4] {
        for k in 0..copies {
            code.extend(i("S_ADD_U32", "ENC_SOP2", &[("SDST", 0), ("SSRC0", 0), ("SSRC1", 1)]));
            code.extend(i("V_ADD_NC_U32", "ENC_VOP2", &[("VDST", 1), ("SRC0", 256), ("VSRC1", 1)]));
            let offset = (copies - 1 - k) * 3 + 1;
            assert!(offset <= i16::MAX as usize);
            code.extend(i("S_CBRANCH_SCC1", "ENC_SOPP", &[("SIMM16", offset as u64)]));
        }
        code.extend(i("S_MOV_B32", "ENC_SOP1", &[("SDST", marker), ("SSRC0", 129)]));
    }
    code.extend(i("S_ENDPGM", "ENC_SOPP", &[]));
    code
}

#[test]
fn a_gpu_with_64_lane_waves_keeps_the_first_table_limit() {
    if compact_mode().is_some() {
        return;
    }
    let code = two_segments(8192);
    let o = KernelOptions { wave64: true, split: false, mem_windows: 1, sampler_base: 0 };
    let cs = crate::gpu::recomp::CsInterface { threads: [64, 1, 1], user_sgprs: 4, tgid: [false; 3], tg_size: false, tid_components: 1 };
    let size = |wave32_only| super::kernel::translate_compute_on(&code, o, &cs, wave32_only).unwrap().dxil.len();
    let (compact, plain) = (size(true), size(false));
    assert!(compact * 20 < plain, "{compact} B on a GPU with 32-lane waves only, {plain} B on one with 64-lane waves");
}

#[test]
fn gpu_compact_inline_table_above_old_cap_checks_late_entries() {
    if compact_mode().is_some() {
        return;
    }
    match (runner::adapter(), runner::wave64_programs_supported(), crate::gpu::recomp::dxc::location()) {
        (Ok(_), Ok(true), Ok(_)) => {}
        (a, _, d) => return runner::skip_test(if a.is_err() { "no GPU" } else if d.is_err() { "no DXC" } else { "no wave64" }),
    }
    const COPIES: usize = 8192;
    let code = two_segments(COPIES);
    let o = KernelOptions { wave64: true, split: runner::split_wave64(), mem_windows: 1, sampler_base: 0 };
    let cfg = super::cfg::Cfg::build(&code).unwrap();
    let leaders = cfg.blocks.iter().filter_map(|b| b.insts.first().map(|&i| cfg.insts[i].pc)).collect();
    let lw = super::lower::Lw::new(&cfg.insts, &leaders, o, &super::kernel::Entry::Kernel);
    let p = super::compact::plan(&cfg, &lw).unwrap();
    assert!(p.inline && p.words() > 8192 && p.words() <= 12288);
    assert!(super::compact::wanted(&cfg, &p, super::compact::TABLE_WORDS_WAVE32));
    let k = translate_full(&code, o).unwrap();
    for initial in [0u32, u32::MAX - 1, 0u32.wrapping_sub(COPIES as u32), 0u32.wrapping_sub((2 * COPIES) as u32), 0u32.wrapping_sub(COPIES as u32 + 1)] {
        let mut st = KernelState::default();
        st.sgpr[0] = initial;
        st.sgpr[1] = 1;
        st.sgpr[2] = 0x1234;
        for lane in 0..KERNEL_LANES {
            st.vgpr[lane][..4].copy_from_slice(&[lane as u32, 17, 0x12345678, 0x87654321]);
        }
        let (mut value, mut count, mut marks, mut carry) = (initial, 0u32, [0u32; 2], false);
        for mark in &mut marks {
            for n in 0..COPIES {
                (value, carry) = value.overflowing_add(1);
                count += 1;
                if carry { break; }
                if n + 1 == COPIES { *mark = 1; }
            }
        }
        let out = runner::run_with_statics(&k.dxil, &st, &[], &[], &[], &k.static_samplers).expect("GPU run");
        assert_eq!(&out.sgpr[..5], &[value, 1, 0x1234, marks[0], marks[1]], "initial {initial:#x}");
        assert_eq!(out.scc, carry as u32);
        for lane in 0..KERNEL_LANES {
            assert_eq!(&out.vgpr[lane][..4], &[lane as u32, 17 + count * lane as u32, 0x12345678, 0x87654321], "initial {initial:#x} lane {lane}");
        }
    }
}
