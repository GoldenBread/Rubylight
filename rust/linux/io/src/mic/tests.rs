//! Tests of the microphone's parts that need no PipeWire daemon, and (ignored) of the whole
//! path against a running one.

use super::device::drain;
use std::collections::VecDeque;

#[test]
fn drain_pads_a_short_ring_with_silence() {
    let mut ring: VecDeque<f32> = [0.5, -0.5, 0.25].into();
    let mut out = [0xffu8; 24];
    let real = drain(&mut ring, &mut out, 5);
    assert_eq!(real, 3);
    assert!(ring.is_empty());
    let samples: Vec<f32> = out
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect();
    assert_eq!(&samples[..5], &[0.5, -0.5, 0.25, 0., 0.]);
    // Past the request nothing is touched.
    assert_eq!(&out[20..], &[0xff; 4]);
}

#[test]
fn drain_keeps_what_the_buffer_cannot_take() {
    let mut ring: VecDeque<f32> = (0..10).map(|i| i as f32).collect();
    let mut out = [0u8; 16];
    assert_eq!(drain(&mut ring, &mut out, 100), 4);
    assert_eq!(ring.len(), 6);
    assert_eq!(ring.front(), Some(&4.));
}

/// What the daemon-backed tests need: a PipeWire daemon with a session manager, and
/// `pw-record`. Run with `cargo test -p butterpollo-linux-io -- --ignored mic::`.
mod daemon {
    use crate::{
        audio_route::{self, MicrophoneRoute},
        mic::{NODE_NAME, Render, graph},
    };
    use butterpollo_session::mic::Playout;
    use std::{
        path::PathBuf,
        process::{Command, Stdio},
        sync::Mutex,
        time::{Duration, Instant},
    };

    /// The tests share one node and one default, so they take turns.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    fn work_dir(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../target/agent-mic/mic-it")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The share of the signal's energy at `freq`, by the Goertzel filter.
    fn tone_share(samples: &[f32], freq: f32) -> f32 {
        let w = 2. * std::f32::consts::PI * freq / 48_000.;
        let (mut s1, mut s2) = (0f32, 0f32);
        for &x in samples {
            let s = x + 2. * w.cos() * s1 - s2;
            s2 = s1;
            s1 = s;
        }
        let power = s1 * s1 + s2 * s2 - 2. * w.cos() * s1 * s2;
        let total: f32 = samples.iter().map(|x| x * x).sum();
        // A pure tone of amplitude A over N samples has power (A N / 2)^2 and energy A^2 N / 2.
        (2. * power / (samples.len() as f32 * total.max(1e-12))).min(1.)
    }

    #[test]
    #[ignore = "needs a PipeWire daemon, a session manager and pw-record"]
    fn a_tone_played_into_the_source_is_recorded_from_it() {
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let dir = work_dir("tone");
        let mut render = Render::open(NODE_NAME).unwrap();
        // The node is listed as a recording device apps can pick.
        let snapshot = graph::snapshot().unwrap();
        let node = snapshot
            .nodes
            .iter()
            .find(|n| n.prop("node.name") == NODE_NAME)
            .expect("the node is in the registry");
        assert_eq!(node.class(), "Audio/Source");
        assert_eq!(node.prop("node.description"), "Rubylight Microphone");
        assert!(node.id > 0);
        assert!(
            audio_route::endpoints()
                .unwrap()
                .iter()
                .any(|e| e.id == NODE_NAME)
        );

        let out = dir.join("record.raw");
        let mut recorder = Command::new("pw-record")
            .args(["--target", NODE_NAME, "--rate", "48000", "--channels", "1"])
            .args(["--format", "f32", "--raw"])
            .arg(&out)
            .stderr(Stdio::null())
            .spawn()
            .expect("pw-record starts");

        // The host's path: 20 ms packets into a shared playout, the render thread's loop.
        let playout = Mutex::new(Playout::default());
        let started = Instant::now();
        let mut phase = 0usize;
        let mut pushed = 0usize;
        let mut max_padding = 0usize;
        while started.elapsed() < Duration::from_millis(3000) {
            // One packet per 20 ms of wall time, as a client would send them.
            while pushed * 1000 < started.elapsed().as_millis() as usize * 48_000 {
                let packet: Vec<f32> = (0..960)
                    .map(|i| {
                        0.5 * (2. * std::f32::consts::PI * 1000. * (phase + i) as f32 / 48_000.)
                            .sin()
                    })
                    .collect();
                phase += 960;
                pushed += 960;
                playout.lock().unwrap().push(&packet);
            }
            render.wait(Duration::from_millis(5));
            render.fill_concealing(&playout, &mut |_| Ok(0)).unwrap();
            max_padding = max_padding.max(render.device.shared.ring().len());
        }
        println!(
            "period {} frames, ring peak {} frames, underruns {}",
            render.period(),
            max_padding,
            playout.lock().unwrap().underruns
        );
        let _ = recorder.kill();
        let _ = recorder.wait();
        let bytes = std::fs::read(&out).unwrap();
        let samples: Vec<f32> = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        assert!(samples.len() > 48_000, "recorded {} samples", samples.len());
        // The middle second, clear of the start and the kill.
        let mid = &samples[samples.len() / 3..samples.len() / 3 + 48_000];
        let share = tone_share(mid, 1000.);
        let peak = mid.iter().fold(0f32, |m, x| m.max(x.abs()));
        println!("1 kHz share {share:.3}, peak {peak:.3}");
        assert!(share > 0.9, "1 kHz holds {share} of the energy");
        assert!((0.4..0.6).contains(&peak), "peak {peak}");
    }

    fn configured() -> Option<String> {
        graph::snapshot()
            .unwrap()
            .default_name(graph::CONFIGURED_SOURCE)
    }

    #[test]
    #[ignore = "needs a PipeWire daemon and a session manager"]
    fn the_route_makes_the_source_the_default_and_puts_the_old_one_back() {
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let dir = work_dir("route");
        let _render = Render::open(NODE_NAME).unwrap();
        let before = configured();
        let route = MicrophoneRoute::acquire(&dir).unwrap();
        assert_eq!(configured().as_deref(), Some(NODE_NAME));
        // The session manager follows: the source in use is ours.
        let resolved = graph::snapshot()
            .unwrap()
            .default_name("default.audio.source");
        println!("before {before:?}, resolved default source {resolved:?}");
        assert_eq!(resolved.as_deref(), Some(NODE_NAME));
        // A second route is refused while the first holds the default.
        assert!(MicrophoneRoute::acquire(&dir).is_err());
        let journal = std::fs::read_to_string(dir.join("microphone-recovery.json")).unwrap();
        assert!(journal.contains(NODE_NAME), "{journal}");
        drop(route);
        assert_eq!(configured(), before);
        assert_eq!(
            std::fs::read_to_string(dir.join("microphone-recovery.json"))
                .unwrap()
                .trim(),
            "null"
        );
    }

    #[test]
    #[ignore = "needs a PipeWire daemon and a session manager"]
    fn recovery_puts_back_the_default_a_crash_left() {
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let dir = work_dir("crash");
        let _render = Render::open(NODE_NAME).unwrap();
        let before = configured();
        // The state a crashed host leaves: our source selected, the journal on disk.
        std::fs::write(
            dir.join("microphone-recovery.json"),
            format!(
                r#"{{"before":{},"applied":"{NODE_NAME}"}}"#,
                before
                    .as_ref()
                    .map_or("null".to_owned(), |name| format!("\"{name}\""))
            ),
        )
        .unwrap();
        graph::session(Some(graph::Change {
            key: graph::CONFIGURED_SOURCE,
            name: Some(NODE_NAME),
        }))
        .unwrap();
        assert_eq!(configured().as_deref(), Some(NODE_NAME));
        audio_route::recover_microphone(&dir).unwrap();
        assert_eq!(configured(), before);
        // Nothing left: a second recovery changes nothing.
        audio_route::recover_microphone(&dir).unwrap();
    }

    #[test]
    #[ignore = "needs a PipeWire daemon"]
    fn the_level_an_app_raised_is_brought_back_to_unity() {
        let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let _render = Render::open(NODE_NAME).unwrap();
        let gain = audio_route::SteamMicrophoneGain::open().unwrap();
        assert_eq!(gain.limit().unwrap(), None);
        let set = |level: &str| {
            Command::new("wpctl")
                .args([
                    "set-volume",
                    "--limit",
                    "1.5",
                    &format!("@{NODE_NAME}"),
                    level,
                ])
                .status()
        };
        // wpctl takes ids, not names: find the node's id.
        let id = graph::snapshot()
            .unwrap()
            .nodes
            .iter()
            .find(|n| n.prop("node.name") == NODE_NAME)
            .map(|n| n.id)
            .unwrap();
        let status = Command::new("wpctl")
            .args(["set-volume", "--limit", "1.5", &id.to_string(), "1.3"])
            .status()
            .unwrap();
        let _ = set;
        assert!(status.success());
        std::thread::sleep(Duration::from_millis(300));
        let lowered = gain.limit().unwrap();
        println!("lowered from {lowered:?} dB");
        assert!(lowered.is_some_and(|db| (db - 6.84).abs() < 0.3));
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(gain.limit().unwrap(), None);
    }
}
