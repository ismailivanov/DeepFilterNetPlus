//! Regression test for https://github.com/Rikorose/DeepFilterNet/issues/661:
//! `run()` is called on the host's real-time thread and must never block,
//! even when the host quantum (e.g. PipeWire min-quantum = 32) is much
//! smaller than the DeepFilterNet hop size.

use std::cell::RefCell;
use std::thread::sleep;
use std::time::{Duration, Instant};

use deep_filter_ladspa::get_ladspa_descriptor;
use deep_filter_ladspa::ladspa::{Plugin, PluginDescriptor, PortConnection, PortData};

const QUANTUM: usize = 32; // PipeWire default min-quantum, see issue #661
const SR: usize = 48000;

/// Run one quantum through the plugin, returning the call duration and whether
/// any non-zero sample was written to the output.
fn run_quantum(plugin: &mut Box<dyn Plugin + Send>, desc: &PluginDescriptor, phase: usize) -> (Duration, bool) {
    let mut in_buf = [0f32; QUANTUM];
    for (j, s) in in_buf.iter_mut().enumerate() {
        // Continuous 440 Hz sine
        *s = 0.5 * (2. * std::f32::consts::PI * 440. * ((phase + j) as f32) / SR as f32).sin();
    }
    let mut out_buf = [0f32; QUANTUM];
    let control_vals = [90f32, -10., 30., 20., 0., 0.];

    let in_port = PortConnection {
        port: desc.ports[0],
        data: PortData::AudioInput(&in_buf),
    };
    let out_port = PortConnection {
        port: desc.ports[1],
        data: PortData::AudioOutput(RefCell::new(&mut out_buf)),
    };
    let c_ports: Vec<PortConnection> = desc.ports[2..]
        .iter()
        .zip(control_vals.iter())
        .map(|(p, v)| PortConnection {
            port: *p,
            data: PortData::ControlInput(v),
        })
        .collect();
    let mut refs: Vec<&PortConnection> = vec![&in_port, &out_port];
    refs.extend(c_ports.iter());

    let t0 = Instant::now();
    plugin.run(QUANTUM, &refs);
    let td = t0.elapsed();
    (td, out_buf.iter().any(|&s| s != 0.))
}

#[test]
fn run_never_blocks_rt_thread() {
    let desc = get_ladspa_descriptor(0).expect("no mono descriptor");
    let mut plugin = (desc.new)(&desc, SR as u64);
    plugin.activate();

    // Phase 1: hammer run() far faster than real-time so the worker is always
    // behind. The old implementation slept >= 2ms per call here; the fix must
    // instead emit silence and return immediately.
    let n = 1200; // keeps latency growth below the 1s panic threshold
    let mut durations = Vec::with_capacity(n);
    for i in 0..n {
        let (td, _) = run_quantum(&mut plugin, &desc, i * QUANTUM);
        durations.push(td);
    }
    durations.sort();
    let median = durations[n / 2];
    assert!(
        median < Duration::from_millis(1),
        "run() blocks the real-time thread: median call duration {median:?}"
    );

    // Phase 2: run paced at ~real-time; processed audio must reach the output.
    let mut any_nonzero = false;
    for i in n..(n + 1500) {
        let (_, nonzero) = run_quantum(&mut plugin, &desc, i * QUANTUM);
        any_nonzero |= nonzero;
        sleep(Duration::from_micros(QUANTUM as u64 * 1_000_000 / SR as u64));
    }
    assert!(any_nonzero, "no processed audio reached the output");
}
