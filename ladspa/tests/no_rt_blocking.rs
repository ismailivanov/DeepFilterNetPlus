//! Regression tests for https://github.com/Rikorose/DeepFilterNet/issues/661:
//! `run()` is called on the host's real-time thread and must never block,
//! even when the host quantum (e.g. PipeWire min-quantum = 32) is much
//! smaller than the DeepFilterNet hop size. Runs both the mono and the
//! stereo plugin: the stereo path exercises multi-channel inference, which
//! broke silently with tract >= 0.21.5 (to_scalar on a (ch,1,1) tensor).

use std::cell::RefCell;
use std::thread::sleep;
use std::time::{Duration, Instant};

use deep_filter_ladspa::get_ladspa_descriptor;
use deep_filter_ladspa::ladspa::{Plugin, PluginDescriptor, PortConnection, PortData};

const QUANTUM: usize = 32; // PipeWire default min-quantum, see issue #661
const SR: usize = 48000;

struct QuantumResult {
    duration: Duration,
    any_nonzero: bool,
    /// Output identical to input indicates the dead-worker passthrough kicked in.
    passthrough: bool,
}

/// Run one quantum of a 440 Hz sine through the plugin.
fn run_quantum(
    plugin: &mut Box<dyn Plugin + Send>,
    desc: &PluginDescriptor,
    ch: usize,
    phase: usize,
) -> QuantumResult {
    let mut in_buf = [0f32; QUANTUM];
    for (j, s) in in_buf.iter_mut().enumerate() {
        *s = 0.5 * (2. * std::f32::consts::PI * 440. * ((phase + j) as f32) / SR as f32).sin();
    }
    let mut out_bufs = vec![[0f32; QUANTUM]; ch];
    let control_vals = [90f32, -10., 30., 20., 0., 0.];

    let in_ports: Vec<PortConnection> = (0..ch)
        .map(|c| PortConnection {
            port: desc.ports[c],
            data: PortData::AudioInput(&in_buf),
        })
        .collect();
    let out_ports: Vec<PortConnection> = out_bufs
        .iter_mut()
        .enumerate()
        .map(|(i, b)| PortConnection {
            port: desc.ports[ch + i],
            data: PortData::AudioOutput(RefCell::new(b)),
        })
        .collect();
    let c_ports: Vec<PortConnection> = desc.ports[2 * ch..]
        .iter()
        .zip(control_vals.iter())
        .map(|(p, v)| PortConnection {
            port: *p,
            data: PortData::ControlInput(v),
        })
        .collect();
    let refs: Vec<&PortConnection> =
        in_ports.iter().chain(out_ports.iter()).chain(c_ports.iter()).collect();

    let t0 = Instant::now();
    plugin.run(QUANTUM, &refs);
    let duration = t0.elapsed();

    let any_nonzero = out_bufs.iter().any(|b| b.iter().any(|&s| s != 0.));
    let passthrough = out_bufs.iter().all(|b| b[..] == in_buf[..]);
    QuantumResult {
        duration,
        any_nonzero,
        passthrough,
    }
}

fn check_plugin(desc_idx: u64, ch: usize) {
    let desc = get_ladspa_descriptor(desc_idx).expect("no descriptor");
    let mut plugin = (desc.new)(&desc, SR as u64);
    plugin.activate();

    // Phase 1: hammer run() far faster than real-time so the worker is always
    // behind. The old implementation slept >= 2ms per call here; the fix must
    // instead emit silence and return immediately.
    let n = 1200; // keeps latency growth below the 1s reset threshold
    let mut durations = Vec::with_capacity(n);
    for i in 0..n {
        durations.push(run_quantum(&mut plugin, &desc, ch, i * QUANTUM).duration);
    }
    durations.sort();
    let median = durations[n / 2];
    assert!(
        median < Duration::from_millis(1),
        "run() blocks the real-time thread: median call duration {median:?}"
    );

    // Phase 2: run paced at ~real-time; processed (not passed-through) audio
    // must reach the output, i.e. the inference worker must be alive.
    let mut any_nonzero = false;
    for i in n..(n + 1500) {
        let r = run_quantum(&mut plugin, &desc, ch, i * QUANTUM);
        if r.any_nonzero {
            any_nonzero = true;
            assert!(
                !r.passthrough,
                "output is a bit-exact copy of the input: worker died, passthrough active"
            );
        }
        sleep(Duration::from_micros(QUANTUM as u64 * 1_000_000 / SR as u64));
    }
    assert!(any_nonzero, "no processed audio reached the output");
}

#[test]
fn run_never_blocks_rt_thread_mono() {
    check_plugin(0, 1);
}

/// Issue #543: hosts running at other sample rates (e.g. 16kHz embedded
/// boards) must get a clean, fast rejection instead of seconds of model
/// loading followed by a leaked worker thread. The ladspa FFI layer catches
/// the panic and reports a failed instantiation to the host.
#[test]
fn unsupported_sample_rate_rejected_before_model_build() {
    let desc = get_ladspa_descriptor(0).expect("no mono descriptor");
    let t0 = Instant::now();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (desc.new)(&desc, 16000)));
    assert!(result.is_err(), "instantiating at 16kHz must be rejected");
    assert!(
        t0.elapsed() < Duration::from_millis(500),
        "rejection must happen before the model is built"
    );
}

#[test]
fn run_never_blocks_rt_thread_stereo() {
    check_plugin(1, 2);
}
