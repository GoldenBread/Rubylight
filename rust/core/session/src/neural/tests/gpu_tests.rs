//! GPU eligibility, multi-GPU selection, Visual C++ runtime check, and probe
//! payload tests for Neural Rendering.
use super::*;

fn streams(vendor: u32, device: u32) -> Adapter {
    adapter("GPU", 7, vendor, device)
}

fn adapter(name: &str, luid: u64, vendor: u32, device: u32) -> Adapter {
    Adapter {
        name: name.into(),
        luid,
        vendor,
        device,
    }
}

fn gpu(arch: &str, luid: Option<u64>) -> Gpu {
    Gpu {
        name: "GPU".into(),
        arch: arch.into(),
        compute_units: 12,
        luid,
    }
}

/// The GPU the host streams with alone.
fn alone(streams: &Adapter, gpus: Result<&[Gpu], &str>) -> Result<Capable, String> {
    capable(streams, std::slice::from_ref(streams), gpus)
}

#[test]
fn the_switch_is_offered_on_the_gpus_the_provider_runs_on() {
    // NVIDIA: Turing (GTX 16, RTX 20) and every newer device ID.
    assert!(alone(&streams(NVIDIA, 0x1e04), Err("no HIP")).is_ok());
    assert!(alone(&streams(NVIDIA, 0x2684), Err("no HIP")).is_ok());
    assert!(alone(&streams(NVIDIA, 0x3100), Err("no HIP")).is_ok());
    let pascal = alone(&streams(NVIDIA, 0x1b80), Err("no HIP")).unwrap_err();
    assert!(pascal.contains("which is older"), "{pascal}");
    // AMD by the compute runtime's architecture.
    for arch in ["gfx1010", "gfx1030", "gfx1103", "gfx1151", "gfx1201"] {
        assert!(
            alone(&streams(AMD, 0xffff), Ok(&[gpu(arch, Some(7))])).is_ok(),
            "{arch}"
        );
    }
    let vega = alone(&streams(AMD, 0x15d8), Ok(&[gpu("gfx90c", Some(7))])).unwrap_err();
    assert!(vega.contains("older"), "{vega}");
    // By name when the runtime gives no LUID, and only when the name is unique.
    assert!(alone(&streams(AMD, 0xffff), Ok(&[gpu("gfx1100", None)])).is_ok());
    // The runtime can't say: the PCI device ID (this laptop's 780M is 0x1900).
    for device in [0x1900, 0x15bf, 0x744c, 0x7550, 0x731f, 0x150e] {
        assert!(
            alone(&streams(AMD, device), Err("no HIP")).is_ok(),
            "{device:#x}"
        );
    }
    let unknown = alone(&streams(AMD, 0x15d8), Err("no HIP")).unwrap_err();
    assert!(
        unknown.contains("can't identify") && unknown.contains("no HIP"),
        "{unknown}"
    );
    let twins = [gpu("gfx1100", None), gpu("gfx1100", None)];
    assert!(
        alone(&streams(AMD, 0x15d8), Ok(&twins))
            .unwrap_err()
            .contains("can't identify")
    );
    // Neither AMD nor NVIDIA.
    assert!(alone(&streams(0x8086, 0x56a0), Err("no HIP")).is_err());
    // The Visual C++ runtime OptiScaler links.
    assert_eq!(runtime(&[]), Ok(()));
    let missing = runtime(&["VCRUNTIME140_1.dll"]).unwrap_err();
    assert!(
        missing.contains("Visual C++") && missing.contains("VCRUNTIME140_1.dll"),
        "{missing}"
    );
}

/// The host can't know which GPU a game renders on: any of the PC's AMD
/// RDNA or NVIDIA Turing-and-newer GPUs admits it, the one it streams with
/// first, and the switch names the GPU and, when it is not the one the host
/// streams with, says frame generation runs when the game renders there.
#[test]
fn any_gpu_of_the_pc_that_runs_it_admits_it() {
    let m780 = adapter("AMD Radeon 780M Graphics", 1, AMD, 0x1900);
    let vega = adapter("AMD Radeon(TM) Graphics", 2, AMD, 0x1638);
    let rtx5060ti = adapter("NVIDIA GeForce RTX 5060 Ti", 3, NVIDIA, 0x2d04);
    let rtx3060 = adapter("NVIDIA GeForce RTX 3060 Laptop GPU", 4, NVIDIA, 0x2560);
    let pascal = adapter("NVIDIA GeForce GTX 1060", 5, NVIDIA, 0x1c03);
    let intel = adapter("Intel(R) UHD Graphics", 6, 0x8086, 0x9a49);
    let rx7900 = adapter("AMD Radeon RX 7900 XT", 8, AMD, 0x744c);
    let hip = |arch: &str, luid: u64| Gpu {
        name: "AMD Radeon".into(),
        arch: arch.into(),
        compute_units: 12,
        luid: Some(luid),
    };
    let runs = |gpu: &Adapter| Capable {
        gpu: gpu.name.clone(),
        streams: None,
    };
    let elsewhere = |gpu: &Adapter, streams: &Adapter| Capable {
        gpu: gpu.name.clone(),
        streams: Some(streams.name.clone()),
    };
    // A 780M that streams beside an RTX 5060 Ti: the 780M runs it.
    let both = [m780.clone(), rtx5060ti.clone()];
    assert_eq!(
        capable(&m780, &both, Ok(&[hip("gfx1103", 1)])),
        Ok(runs(&m780))
    );
    assert_eq!(
        capable(&rtx5060ti, &both, Err("no HIP")),
        Ok(runs(&rtx5060ti))
    );
    // A Vega iGPU (gfx90c) that streams beside an RTX 3060: the RTX, named.
    let hybrid = [vega.clone(), rtx3060.clone()];
    let found = capable(&vega, &hybrid, Ok(&[hip("gfx90c", 2)])).unwrap();
    assert_eq!(found, elsewhere(&rtx3060, &vega));
    let note = found.note().unwrap();
    assert!(
        note.contains("runs when the game renders on NVIDIA GeForce RTX 3060")
            && note.contains("streams with AMD Radeon(TM) Graphics"),
        "{note}"
    );
    assert_eq!(runs(&vega).note(), None);
    // An RDNA iGPU beside a Pascal card: the iGPU, whichever streams.
    let rdna_pascal = [m780.clone(), pascal.clone()];
    assert_eq!(capable(&m780, &rdna_pascal, Err("no HIP")), Ok(runs(&m780)));
    assert_eq!(
        capable(&pascal, &rdna_pascal, Err("no HIP")),
        Ok(elsewhere(&m780, &pascal))
    );
    // Intel graphics that stream beside an NVIDIA card: the NVIDIA card.
    let intel_nvidia = [intel.clone(), rtx3060.clone()];
    assert_eq!(
        capable(&intel, &intel_nvidia, Err("no HIP")),
        Ok(elsewhere(&rtx3060, &intel))
    );
    // Intel never counts, nor a GPU of another vendor.
    let intel_pascal = [intel.clone(), pascal.clone()];
    let refused = capable(&intel, &intel_pascal, Err("no HIP")).unwrap_err();
    assert!(
        refused.contains("streams with Intel(R) UHD Graphics")
            && refused.contains("also has NVIDIA GeForce GTX 1060, which is older"),
        "{refused}"
    );
    assert!(capable(&pascal, &[intel.clone(), pascal.clone()], Err("no HIP")).is_err());
    // A single AMD GPU, and a single Pascal card.
    assert_eq!(
        capable(
            &rx7900,
            std::slice::from_ref(&rx7900),
            Ok(&[hip("gfx1100", 8)])
        ),
        Ok(runs(&rx7900))
    );
    let older = capable(&pascal, std::slice::from_ref(&pascal), Err("no HIP")).unwrap_err();
    assert!(
        older.contains("GTX 1060, which is older") && !older.contains("also"),
        "{older}"
    );
    // Two Vega iGPU and Pascal: both named.
    let neither = capable(
        &vega,
        &[vega.clone(), pascal.clone()],
        Ok(&[hip("gfx90c", 2)]),
    )
    .unwrap_err();
    assert!(
        neither.contains("(gfx90c), which is older")
            && neither.contains("also has NVIDIA GeForce GTX 1060"),
        "{neither}"
    );
    // Offered: any AMD or NVIDIA GPU, by vendor alone.
    assert!(offered(&hybrid) && offered(&intel_nvidia) && offered(std::slice::from_ref(&pascal)));
    assert!(!offered(std::slice::from_ref(&intel)) && !offered(&[]));
    // The compute runtime is asked only when an AMD GPU is there and the
    // GPU that streams is no NVIDIA one that runs it.
    assert!(needs_probe(&vega, &hybrid) && needs_probe(&pascal, &rdna_pascal));
    assert!(!needs_probe(&rtx5060ti, &both) && !needs_probe(&intel, &intel_nvidia));
}

/// A streaming GPU that can't be found leaves the PC's other GPUs judged,
/// with no note, as the host can't tell which one it streams with.
#[test]
fn an_unknown_streaming_gpu_still_judges_the_others() {
    let m780 = adapter("AMD Radeon 780M Graphics", 1, AMD, 0x1900);
    let rtx3060 = adapter("NVIDIA GeForce RTX 3060 Laptop GPU", 4, NVIDIA, 0x2560);
    let pascal = adapter("NVIDIA GeForce GTX 1060", 5, NVIDIA, 0x1c03);
    let intel = adapter("Intel(R) UHD Graphics", 6, 0x8086, 0x9a49);
    let hip = |arch: &str, luid: u64| Gpu {
        name: "AMD Radeon".into(),
        arch: arch.into(),
        compute_units: 12,
        luid: Some(luid),
    };
    let unknown = "the GPU Rubylight streams with can't be found";
    let found = capable_any(&[intel.clone(), rtx3060.clone()], Err("no HIP"), unknown).unwrap();
    assert_eq!(found.gpu, rtx3060.name);
    assert_eq!(found.note(), None);
    assert_eq!(
        capable_any(
            std::slice::from_ref(&m780),
            Ok(&[hip("gfx1103", 1)]),
            unknown
        )
        .unwrap()
        .gpu,
        m780.name
    );
    let refused =
        capable_any(&[intel.clone(), pascal.clone()], Err("no HIP"), unknown).unwrap_err();
    assert!(
        refused.contains(unknown)
            && refused.contains("it has NVIDIA GeForce GTX 1060, which is older"),
        "{refused}"
    );
    assert!(
        capable_any(&[], Err("no HIP"), unknown)
            .unwrap_err()
            .contains(unknown)
    );
}

#[test]
fn probe_gpus_propagates_error_only_when_no_gpus_were_reported() {
    assert_eq!(
        probe_gpus(Probe {
            gpus: Vec::new(),
            error: Some("no hip".into()),
        }),
        Err("no hip".into())
    );
    let list = vec![gpu("gfx1100", Some(1))];
    assert_eq!(
        probe_gpus(Probe {
            gpus: list.clone(),
            error: Some("partial".into()),
        }),
        Ok(list)
    );
}
