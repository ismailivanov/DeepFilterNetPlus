use std::collections::VecDeque;
use std::fmt;
use std::io::{self, Write};
use std::sync::{
    Arc, Mutex, Once,
    mpsc::{Receiver, SyncSender, sync_channel},
};
use std::thread::{self, JoinHandle, sleep};
use std::time::{Duration, Instant};

use df::tract::*;
pub use ladspa;
use ladspa::{DefaultValue, Plugin, PluginDescriptor, Port, PortConnection, PortDescriptor};
use ndarray::prelude::*;
use uuid::Uuid;

static INIT_LOGGER: Once = Once::new();

/// Wireless receivers and PipeWire graph discontinuities can inject garbage
/// into the stream (observed in the field on a USB wireless headset: NaN
/// bursts and float-max ±3.4e38 samples). NaN poisons the DSP for over a
/// second and float-max values are full-scale pops, so anything outside a
/// generous audio range is dropped to silence at every copy boundary.
/// Real audio is |s| <= ~1; 16 (+24 dBFS) leaves headroom for hot chains.
#[inline]
fn finite_or_zero(s: f32) -> f32 {
    if s.is_finite() && s.abs() <= 16. { s } else { 0. }
}

type SampleQueue = Arc<Mutex<Vec<VecDeque<f32>>>>;
type ControlProd = SyncSender<(DfControl, f32)>;
type ControlRecv = Receiver<(DfControl, f32)>;
#[cfg(feature = "dbus")]
use ::{
    event_listener::Event,
    zbus::{blocking::ConnectionBuilder, dbus_interface},
};
#[cfg(feature = "dbus")]
const DBUS_NAME: &str = "org.deepfilter.DeepFilterLadspa";
#[cfg(feature = "dbus")]
const DBUS_PATH: &str = "/org/deepfilter/DeepFilterLadspa";

const ATTEN_LIM_DEF: DefaultValue = DefaultValue::Maximum;
const ATTEN_LIM_MIN: f32 = 0.;
const ATTEN_LIM_MAX: f32 = 100.;
const PF_BETA_DEF: DefaultValue = DefaultValue::Minimum;
const PF_BETA_MIN: f32 = 0.;
const PF_BETA_MAX: f32 = 0.05;
const MIN_PROC_THRESH_DEF: DefaultValue = DefaultValue::Minimum;
const MIN_PROC_THRESH_MIN: f32 = -15.;
const MIN_PROC_THRESH_MAX: f32 = 35.;
const MAX_ERB_BUF_DEF: DefaultValue = DefaultValue::Maximum;
const MAX_ERB_BUF_MIN: f32 = -15.;
const MAX_ERB_BUF_MAX: f32 = 35.;
const MAX_DF_BUF_DEF: DefaultValue = DefaultValue::Maximum;
const MAX_DF_BUF_MIN: f32 = -15.;
const MAX_DF_BUF_MAX: f32 = 35.;
const MIN_PROC_BUF_DEF: DefaultValue = DefaultValue::Minimum;
const MIN_PROC_BUF_MIN: f32 = 0.;
const MIN_PROC_BUF_MAX: f32 = 10.;

// Overload circuit breaker: after this many backlog resets within one overload
// episode the worker clearly cannot keep up; pass audio through unprocessed
// for the cooldown instead of rebuilding a one-second silence staircase every
// second (see docs/LADSPA_OVERLOAD_REPORT.md).
const OVERLOAD_RESETS_TO_BYPASS: u32 = 3;
const OVERLOAD_BYPASS_SECS: usize = 5;

struct DfPlugin {
    i_tx: SampleQueue,
    o_rx: SampleQueue,
    control_tx: ControlProd,
    id: String,
    ch: usize,
    sr: usize,
    frame_size: usize,
    proc_delay: usize,
    t_proc_change: usize,
    min_q_level: usize, // Lowest output queue level since the last latency change
    worker_dead: bool,
    in_overload: bool,      // Inside an overload episode (episode = until 1s clean)
    ep_underruns: u32,      // Underruns in the current episode
    ep_resets: u32,         // Backlog resets in the current episode
    ep_samples: usize,      // Episode duration in samples
    t_clean: usize,         // Samples since the last underrun
    bypass_remaining: usize, // Samples left in overload-bypass cooldown
    control_hist: DfControlHistory,
    _h: JoinHandle<()>, // Worker thread handle
    #[cfg(feature = "dbus")]
    _dbus: Option<(JoinHandle<()>, Arc<Event>)>, // dbus thread handle
}

const ID_MONO: u64 = 7843795;
const ID_STEREO: u64 = 7843796;

fn log_format(buf: &mut env_logger::fmt::Formatter, record: &log::Record) -> io::Result<()> {
    let ts = buf.timestamp_millis();
    let module = if let Some(m) = record.module_path() {
        format!(" {} |", m.replace("::reexport_dataset_modules:", ""))
    } else {
        "".to_string()
    };
    let level_style = buf.default_level_style(log::Level::Info);

    writeln!(
        buf,
        "{} | {} | {} {}",
        ts,
        level_style.value(record.level()),
        module,
        record.args()
    )
}

fn syslog_format(buf: &mut env_logger::fmt::Formatter, record: &log::Record) -> io::Result<()> {
    writeln!(
        buf,
        "<{}>{}: {}",
        match record.level() {
            log::Level::Error => 3,
            log::Level::Warn => 4,
            log::Level::Info => 6,
            log::Level::Debug => 7,
            log::Level::Trace => 7,
        },
        record.target(),
        record.args()
    )
}

fn get_worker_fn(
    channels: usize,
    inqueue: SampleQueue,
    outqueue: SampleQueue,
    controls: ControlRecv,
    id: String,
    init_tx: SyncSender<(usize, usize)>,
) -> impl FnMut() {
    move || {
        // DfTract is not Send, so the model is built here, inside the worker
        // thread. Doing this eagerly (instead of on the first frame) keeps the
        // worker responsive as soon as `new()` returns.
        let df_params = DfParams::default();
        let r_params = RuntimeParams::default_with_ch(channels);
        let mut df =
            DfTract::new(df_params, &r_params).expect("Could not initialize DeepFilter runtime");
        init_tx.send((df.sr, df.hop_size)).expect("Failed to report model parameters");
        // Poll interval bounds the input-side latency jitter; 0.5ms keeps it
        // negligible against the 10ms frame budget at trivial wakeup cost.
        let sleep_duration = Duration::from_secs_f32(df.hop_size as f32 / df.sr as f32 / 20.);
        let mut inframe = Array2::zeros((df.ch, df.hop_size));
        let mut outframe = Array2::zeros((df.ch, df.hop_size));
        let t_audio_ms = df.hop_size as f32 / df.sr as f32 * 1000.;
        loop {
            if let Ok((c, v)) = controls.try_recv() {
                log::info!("DF {} | Setting '{}' to {:.1}", id, c, v);
                match c {
                    DfControl::AttenLim => df.set_atten_lim(v),
                    DfControl::PfBeta => df.set_pf_beta(v),
                    DfControl::MinThreshDb => df.min_db_thresh = v,
                    DfControl::MaxErbThreshDb => df.max_db_erb_thresh = v,
                    DfControl::MaxDfThreshDb => df.max_db_df_thresh = v,
                    _ => (),
                }
            }
            let got_samples = {
                let mut q = inqueue.lock().unwrap();
                if q[0].len() >= df.hop_size {
                    for (i_q_ch, mut i_ch) in q.iter_mut().zip(inframe.outer_iter_mut()) {
                        for (i, s) in i_ch.iter_mut().zip(i_q_ch.drain(..df.hop_size)) {
                            *i = s;
                        }
                    }
                    true
                } else {
                    false
                }
            };
            if !got_samples {
                sleep(sleep_duration);
                continue;
            }
            let t0 = Instant::now();
            let lsnr = df
                .process(inframe.view(), outframe.view_mut())
                .expect("Error during df::process");
            {
                let mut o_q = outqueue.lock().unwrap();
                for (o_ch, o_q_ch) in outframe.outer_iter().zip(o_q.iter_mut()) {
                    // Second line of defense: never hand NaN to the graph even
                    // if the DSP produced it internally.
                    o_q_ch.extend(o_ch.iter().map(|&s| finite_or_zero(s)));
                }
            }
            let td_ms = t0.elapsed().as_secs_f32() * 1000.;
            log::debug!(
                "DF {} | Enhanced {:.1}ms frame. SNR: {:>5.1}, Processing time: {:>4.1}ms, RTF: {:.2}",
                id,
                t_audio_ms,
                lsnr,
                td_ms,
                td_ms / t_audio_ms
            );
        }
    }
}

fn get_new_df(channels: usize) -> impl Fn(&PluginDescriptor, u64) -> DfPlugin {
    move |_: &PluginDescriptor, sample_rate: u64| {
        let t0 = Instant::now();
        let f = match std::env::var("RUST_LOG_STYLE") {
            Ok(s) if s == "SYSTEMD" => syslog_format,
            _ => log_format,
        };
        INIT_LOGGER.call_once(|| {
            env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
                .filter_module("polling", log::LevelFilter::Error)
                .filter_module("async_io", log::LevelFilter::Error)
                .filter_module("tract_onnx", log::LevelFilter::Error)
                .filter_module("tract_core", log::LevelFilter::Error)
                .filter_module("tract_hir", log::LevelFilter::Error)
                .filter_module("tract_linalg", log::LevelFilter::Error)
                .format(f)
                .init();
        });

        // The bundled model is fixed at 48kHz. Check before building the model:
        // this panic is caught by the ladspa FFI layer (the host just skips the
        // plugin), while the old post-init assert wasted seconds of model
        // loading and leaked the worker thread first (issue #543).
        if sample_rate != 48000 {
            log::error!(
                "DeepFilter requires a 48000 Hz host sample rate, got {sample_rate} Hz. \
                 Configure your sound server to run at 48 kHz."
            );
            panic!("DeepFilter: unsupported sample rate {sample_rate}");
        }

        let i_tx = Arc::new(Mutex::new(vec![VecDeque::new(); channels]));
        let o_rx = Arc::new(Mutex::new(vec![VecDeque::new(); channels]));
        let id = Uuid::new_v4().as_urn().to_string().split_at(33).1.to_string();

        let (control_tx, control_rx) = sync_channel(32);
        let (init_tx, init_rx) = sync_channel(1);

        let worker_handle = thread::spawn(get_worker_fn(
            channels,
            Arc::clone(&i_tx),
            Arc::clone(&o_rx),
            control_rx,
            id.clone(),
            init_tx,
        ));
        // Block until the worker has built the model; instantiation does not
        // happen on the real-time thread.
        let (m_sr, hop) = init_rx.recv().expect("DF worker failed to initialize");
        assert_eq!(m_sr as u64, sample_rate, "Unsupported sample rate");
        let frame_size = hop;
        let proc_delay = hop;
        // Add a buffer of 1 frame to compensate processing delays causing underruns
        for o_ch in o_rx.lock().unwrap().iter_mut() {
            for _ in 0..proc_delay {
                o_ch.push_back(0f32)
            }
        }
        let hist = DfControlHistory::default();
        log::info!(
            "DF {} | Initialized plugin in {:.1}ms",
            &id,
            t0.elapsed().as_secs_f32() * 1000.
        );
        DfPlugin {
            i_tx,
            o_rx,
            control_tx,
            ch: channels,
            sr: m_sr,
            id,
            frame_size,
            proc_delay,
            t_proc_change: 0,
            min_q_level: usize::MAX,
            worker_dead: false,
            in_overload: false,
            ep_underruns: 0,
            ep_resets: 0,
            ep_samples: 0,
            t_clean: 0,
            bypass_remaining: 0,
            control_hist: hist,
            _h: worker_handle,
            #[cfg(feature = "dbus")]
            _dbus: None,
        }
    }
}

#[derive(PartialEq, Clone, Copy)]
enum DfControl {
    AttenLim,
    PfBeta,
    MinThreshDb,
    MaxErbThreshDb,
    MaxDfThreshDb,
    MinBufferFrames,
}
impl DfControl {
    fn from_port_name(name: &str) -> Self {
        match name {
            "Attenuation Limit (dB)" => Self::AttenLim,
            "Post Filter Beta" => Self::PfBeta,
            "Min processing threshold (dB)" => Self::MinThreshDb,
            "Max ERB processing threshold (dB)" => Self::MaxErbThreshDb,
            "Max DF processing threshold (dB)" => Self::MaxDfThreshDb,
            "Min Processing Buffer (frames)" => Self::MinBufferFrames,
            _ => panic!("name not found"),
        }
    }
}
impl fmt::Display for DfControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DfControl::AttenLim => write!(f, "Attenuation Limit (dB)"),
            DfControl::PfBeta => write!(f, "Post Filter Beta"),
            DfControl::MinThreshDb => write!(f, "Min processing threshold (dB)"),
            DfControl::MaxErbThreshDb => write!(f, "Max ERB processing threshold (dB)"),
            DfControl::MaxDfThreshDb => write!(f, "Max DF processing threshold (dB)"),
            DfControl::MinBufferFrames => write!(f, "Min Processing Buffer (frames)"),
        }
    }
}

struct DfControlHistory {
    atten_lim: f32,
    pf_beta: f32,
    min_thresh_db: f32,
    max_erb_thresh_db: f32,
    max_df_thresh_db: f32,
    min_buffer_frames: f32,
}
impl Default for DfControlHistory {
    fn default() -> Self {
        Self {
            atten_lim: 100.,
            pf_beta: 0.0,
            min_thresh_db: -10.,
            max_erb_thresh_db: 30.,
            max_df_thresh_db: 20.,
            min_buffer_frames: 0.,
        }
    }
}
impl DfControlHistory {
    fn get(&self, c: &DfControl) -> f32 {
        match c {
            DfControl::AttenLim => self.atten_lim,
            DfControl::PfBeta => self.pf_beta,
            DfControl::MinThreshDb => self.min_thresh_db,
            DfControl::MaxErbThreshDb => self.max_erb_thresh_db,
            DfControl::MaxDfThreshDb => self.max_df_thresh_db,
            DfControl::MinBufferFrames => self.min_buffer_frames,
        }
    }
    fn set(&mut self, c: &DfControl, v: f32) {
        match c {
            DfControl::AttenLim => self.atten_lim = v,
            DfControl::PfBeta => self.pf_beta = v,
            DfControl::MinThreshDb => self.min_thresh_db = v,
            DfControl::MaxErbThreshDb => self.max_erb_thresh_db = v,
            DfControl::MaxDfThreshDb => self.max_df_thresh_db = v,
            DfControl::MinBufferFrames => self.min_buffer_frames = v,
        }
    }
}

impl Plugin for DfPlugin {
    fn activate(&mut self) {
        log::info!("DF {} | activate", self.id);
        #[cfg(feature = "dbus")]
        {
            if !test_dbus_name_avail() {
                return;
            }
            let init = Arc::new(Event::new());
            let done = Arc::new(Event::new());
            let init_listen = init.listen();
            self._dbus = Some((
                thread::spawn(get_dbus_worker(
                    self.control_tx.clone(),
                    init,
                    done.clone(),
                    self.id.clone(),
                )),
                done,
            ));
            init_listen.wait(); // Wait for dbus server init
            log::debug!("dbus thread spawned")
        }
    }
    fn deactivate(&mut self) {
        log::info!("DF {} | deactivate", self.id);
        #[cfg(feature = "dbus")]
        {
            if let Some((handle, done)) = self._dbus.take() {
                done.notify(1);
                for _ in 0..20 {
                    sleep(Duration::from_millis(5));
                    if handle.is_finished() {
                        match handle.join() {
                            Ok(_) => log::debug!("{} | dbus thread joined", self.id),
                            Err(e) => log::error!("{} | dbus thread error: {:?}", self.id, e),
                        }
                        break;
                    }
                    log::error!("{} | Joining dbus thread timed out.", self.id);
                }
            }
        }
    }
    fn run<'a>(&mut self, sample_count: usize, ports: &[&'a PortConnection<'a>]) {
        let mut i = 0;
        let mut inputs = Vec::with_capacity(self.ch);
        let mut outputs = Vec::with_capacity(self.ch);
        for _ in 0..self.ch {
            inputs.push(ports[i].unwrap_audio());
            i += 1;
        }
        for _ in 0..self.ch {
            outputs.push(ports[i].unwrap_audio_mut());
            i += 1;
        }

        // If the worker died (e.g. a model inference error), pass audio through
        // unprocessed instead of emitting silence forever. Also skips the
        // control channel whose receiver is gone.
        if self.worker_dead || self._h.is_finished() {
            if !self.worker_dead {
                self.worker_dead = true;
                log::error!(
                    "DF {} | Worker thread died; passing audio through unprocessed",
                    self.id
                );
            }
            for (i_ch, o_ch) in inputs.iter().zip(outputs.iter_mut()) {
                for (&i, o) in i_ch.iter().zip(o_ch.iter_mut()) {
                    *o = finite_or_zero(i)
                }
            }
            return;
        }

        // Overload-bypass cooldown: the worker proved it cannot keep up, so
        // give it (and the system) a break. Unprocessed audio beats the
        // silence the overload path would emit otherwise. The input queue is
        // not fed, so the worker idles at its poll sleep.
        if self.bypass_remaining > 0 {
            self.bypass_remaining = self.bypass_remaining.saturating_sub(sample_count);
            for (i_ch, o_ch) in inputs.iter().zip(outputs.iter_mut()) {
                for (&i, o) in i_ch.iter().zip(o_ch.iter_mut()) {
                    *o = finite_or_zero(i)
                }
            }
            if self.bypass_remaining == 0 {
                // Re-arm the processed path exactly like at plugin init. The
                // extra clear also flushes any stale in-flight frame the
                // worker pushed after the queues were dropped.
                for i_ch in self.i_tx.lock().unwrap().iter_mut() {
                    i_ch.clear();
                }
                for o_ch in self.o_rx.lock().unwrap().iter_mut() {
                    o_ch.clear();
                    o_ch.extend(std::iter::repeat(0f32).take(self.frame_size));
                }
                self.proc_delay = self.frame_size;
                self.t_proc_change = 0;
                self.min_q_level = usize::MAX;
                log::info!("DF {} | Overload cooldown over, resuming processing", self.id);
            }
            return;
        }

        for p in ports[i..].iter() {
            let &v = p.unwrap_control();
            let c = DfControl::from_port_name(p.port.name);
            if c == DfControl::AttenLim && v >= 100. {
                for (i_ch, o_ch) in inputs.iter().zip(outputs.iter_mut()) {
                    for (&i, o) in i_ch.iter().zip(o_ch.iter_mut()) {
                        *o = finite_or_zero(i)
                    }
                }
            }
            // try_send, never send: a blocked control channel must not stall
            // the real-time thread. On a full channel the history is left
            // unchanged, so the value is retried on the next run().
            if v != self.control_hist.get(&c) && self.control_tx.try_send((c, v)).is_ok() {
                self.control_hist.set(&c, v);
            }
        }

        {
            let i_q = &mut self.i_tx.lock().unwrap();
            for (i_ch, i_q_ch) in inputs.iter().zip(i_q.iter_mut()) {
                i_q_ch.extend(i_ch.iter().map(|&s| finite_or_zero(s)));
            }
        }

        // Never block the real-time thread waiting for the worker (issue #661):
        // with small host quanta (e.g. PipeWire's min-quantum of 32) the deadline
        // is far below one sleep interval, and blocking here causes xruns for the
        // whole audio graph. If the worker is not done yet, emit silence instead
        // and grow the processing latency by the missing amount.
        let underrun = {
            let o_q = &mut self.o_rx.lock().unwrap();
            if o_q[0].len() >= sample_count {
                for (o_q_ch, o_ch) in o_q.iter_mut().zip(outputs.iter_mut()) {
                    for (o, s) in o_ch.iter_mut().zip(o_q_ch.drain(..sample_count)) {
                        *o = s;
                    }
                }
                self.min_q_level = self.min_q_level.min(o_q[0].len());
                false
            } else {
                for o_ch in outputs.iter_mut() {
                    for o in o_ch.iter_mut() {
                        *o = 0.;
                    }
                }
                true
            }
        };

        if underrun {
            // Logging is bounded per overload episode instead of per step: the
            // old per-underrun warnings produced ~48 journal lines per second
            // under sustained overload, feeding the very pressure that caused
            // the overload (see docs/LADSPA_OVERLOAD_REPORT.md).
            if !self.in_overload {
                self.in_overload = true;
                self.ep_underruns = 0;
                self.ep_resets = 0;
                self.ep_samples = 0;
                log::warn!(
                    "DF {} | Output underrun, increasing processing latency (one log per overload episode)",
                    self.id
                );
            }
            self.ep_underruns += 1;
            self.t_clean = 0;
            if self.proc_delay >= self.sr {
                // Sustained overload: no buffer size can fix this. Drop the
                // stale backlog and restart from the initial latency.
                self.ep_resets += 1;
                for i_ch in self.i_tx.lock().unwrap().iter_mut() {
                    i_ch.clear();
                }
                if self.ep_resets >= OVERLOAD_RESETS_TO_BYPASS {
                    // Circuit breaker: repeated resets mean the staircase will
                    // just repeat. Bypass instead of emitting more silence.
                    log::error!(
                        "DF {} | Processing too slow ({} underruns, {} backlog resets in {:.1}s), \
                         passing audio through unprocessed for {}s. \
                         Try to decrease 'Max DF processing threshold (dB)'.",
                        self.id,
                        self.ep_underruns,
                        self.ep_resets,
                        self.ep_samples as f32 / self.sr as f32,
                        OVERLOAD_BYPASS_SECS,
                    );
                    for o_ch in self.o_rx.lock().unwrap().iter_mut() {
                        o_ch.clear();
                    }
                    self.bypass_remaining = OVERLOAD_BYPASS_SECS * self.sr;
                    self.in_overload = false;
                    // This quantum was zeroed above; pass it through instead.
                    for (i_ch, o_ch) in inputs.iter().zip(outputs.iter_mut()) {
                        for (&i, o) in i_ch.iter().zip(o_ch.iter_mut()) {
                            *o = finite_or_zero(i)
                        }
                    }
                } else {
                    for o_ch in self.o_rx.lock().unwrap().iter_mut() {
                        o_ch.clear();
                        o_ch.extend(std::iter::repeat(0f32).take(self.frame_size));
                    }
                    self.proc_delay = self.frame_size;
                }
            } else {
                // The silence emitted above already delays the queued samples by
                // sample_count. With small host quanta that converges too slowly
                // (one warning every few seconds while latency creeps up), so
                // grow by at least one full frame by extending the current gap.
                let extra = self.frame_size.saturating_sub(sample_count);
                for o_ch in self.o_rx.lock().unwrap().iter_mut() {
                    for _ in 0..extra {
                        o_ch.push_front(0f32);
                    }
                }
                self.proc_delay += sample_count + extra;
            }
            self.t_proc_change = 0;
            self.min_q_level = usize::MAX;
        } else if self.in_overload && self.t_clean >= self.sr {
            // One second without underruns closes the episode.
            self.in_overload = false;
            log::info!(
                "DF {} | Overload ended: {} underruns, {} backlog resets, {:.1}s, latency {:.1}ms",
                self.id,
                self.ep_underruns,
                self.ep_resets,
                self.ep_samples as f32 / self.sr as f32,
                self.proc_delay as f32 * 1000. / self.sr as f32
            );
        } else if self.t_proc_change > 10 * self.sr
            && self.proc_delay
                >= self.frame_size * (1 + self.control_hist.min_buffer_frames as usize)
            && self.min_q_level >= self.frame_size
        {
            // No underrun for 10s and even the worst-case queue level kept a
            // full spare frame: that margin is unused, trim it.
            {
                let o_q = &mut self.o_rx.lock().unwrap();
                for o_q_ch in o_q.iter_mut() {
                    // Cannot underflow: the queue never dropped below frame_size
                    // and the worker only ever adds samples.
                    o_q_ch.drain(..self.frame_size);
                }
            }
            self.proc_delay -= self.frame_size;
            self.t_proc_change = 0;
            self.min_q_level = usize::MAX;
            log::info!(
                "DF {} | Decreasing processing latency to {:.1}ms",
                self.id,
                self.proc_delay as f32 * 1000. / self.sr as f32
            );
        }
        // Counts samples since the last latency change, independent of quantum size
        self.t_proc_change += sample_count;
        if !underrun {
            self.t_clean += sample_count;
        }
        if self.in_overload {
            self.ep_samples += sample_count;
        }
    }
}

#[cfg(feature = "dbus")]
fn build_dbus_session<I>(control: I) -> Result<zbus::blocking::Connection, zbus::Error>
where
    I: zbus::Interface,
{
    ConnectionBuilder::session()?
        .name(DBUS_NAME)?
        .serve_at(DBUS_PATH, control)?
        .build()
}
#[cfg(feature = "dbus")]
fn test_dbus_name_avail() -> bool {
    let control = DfDbusControlDummy {};
    match build_dbus_session(control) {
        Ok(con) => {
            con.release_name(DBUS_NAME).expect("Failed to release dbus name");
            true
        }
        Err(e) => {
            log::error!("Failed to init dbus session {}", e);
            false
        }
    }
}

#[cfg(feature = "dbus")]
fn get_dbus_worker(
    tx: ControlProd,
    init: Arc<Event>,
    done: Arc<Event>,
    id: String,
) -> impl FnMut() {
    move || {
        log::debug!("{id} | Initializing dbus server");
        let done_listener = done.clone().listen();
        let control = DfDbusControl { tx: tx.clone() };
        let con = build_dbus_session(control).expect("Failed to init dbus session");
        init.notify(1); // Notify caller that dbus server has been initialized.
        done_listener.wait();
        con.release_name(DBUS_NAME).expect("Failed to release dbus name");
        log::debug!("{id} | Got done notification. Releasing dbus name");
    }
}

#[cfg(feature = "dbus")]
struct DfDbusControlDummy {}
#[cfg(feature = "dbus")]
#[dbus_interface(name = "org.deepfilter.DeepFilterLadspa")]
impl DfDbusControlDummy {}

#[cfg(feature = "dbus")]
struct DfDbusControl {
    tx: ControlProd,
}

#[cfg(feature = "dbus")]
#[dbus_interface(name = "org.deepfilter.DeepFilterLadspa")]
impl DfDbusControl {
    fn atten_lim(&self, lim: u32) {
        self.tx
            .send((DfControl::AttenLim, lim as f32))
            .expect("Failed to send DfControl");
    }
    fn pf_beta(&self, beta: f32) {
        self.tx.send((DfControl::PfBeta, beta)).expect("Failed to send DfControl");
    }
    fn min_processing_thresh(&self, thresh: i32) {
        self.tx
            .send((DfControl::MinThreshDb, thresh as f32))
            .expect("Failed to send DfControl")
    }
    fn max_erb_thresh(&self, thresh: i32) {
        self.tx
            .send((DfControl::MaxErbThreshDb, thresh as f32))
            .expect("Failed to send DfControl")
    }
    fn max_df_thresh(&self, thresh: i32) {
        self.tx
            .send((DfControl::MaxDfThreshDb, thresh as f32))
            .expect("Failed to send DfControl")
    }
}

#[unsafe(no_mangle)]
pub fn get_ladspa_descriptor(index: u64) -> Option<PluginDescriptor> {
    let descriptor = match index {
        0 => PluginDescriptor {
            unique_id: ID_MONO,
            label: "deep_filter_mono",
            properties: ladspa::Properties::PROP_NONE,
            name: "DeepFilter Mono",
            maker: "Hendrik Schröter",
            copyright: "MIT/Apache",
            ports: vec![
                Port {
                    name: "Audio In",
                    desc: PortDescriptor::AudioInput,
                    ..Default::default()
                },
                Port {
                    name: "Audio Out",
                    desc: PortDescriptor::AudioOutput,
                    ..Default::default()
                },
                Port {
                    name: "Attenuation Limit (dB)",
                    desc: PortDescriptor::ControlInput,
                    hint: None,
                    default: Some(ATTEN_LIM_DEF),
                    lower_bound: Some(ATTEN_LIM_MIN),
                    upper_bound: Some(ATTEN_LIM_MAX),
                },
                Port {
                    name: "Min processing threshold (dB)",
                    desc: PortDescriptor::ControlInput,
                    hint: None,
                    default: Some(MIN_PROC_THRESH_DEF),
                    lower_bound: Some(MIN_PROC_THRESH_MIN),
                    upper_bound: Some(MIN_PROC_THRESH_MAX),
                },
                Port {
                    name: "Max ERB processing threshold (dB)",
                    desc: PortDescriptor::ControlInput,
                    hint: None,
                    default: Some(MAX_ERB_BUF_DEF),
                    lower_bound: Some(MAX_ERB_BUF_MIN),
                    upper_bound: Some(MAX_ERB_BUF_MAX),
                },
                Port {
                    name: "Max DF processing threshold (dB)",
                    desc: PortDescriptor::ControlInput,
                    hint: None,
                    default: Some(MAX_DF_BUF_DEF),
                    lower_bound: Some(MAX_DF_BUF_MIN),
                    upper_bound: Some(MAX_DF_BUF_MAX),
                },
                Port {
                    name: "Min Processing Buffer (frames)",
                    desc: PortDescriptor::ControlInput,
                    hint: None,
                    default: Some(MIN_PROC_BUF_DEF),
                    lower_bound: Some(MIN_PROC_BUF_MIN),
                    upper_bound: Some(MIN_PROC_BUF_MAX),
                },
                Port {
                    name: "Post Filter Beta",
                    desc: PortDescriptor::ControlInput,
                    hint: None,
                    default: Some(PF_BETA_DEF),
                    lower_bound: Some(PF_BETA_MIN),
                    upper_bound: Some(PF_BETA_MAX),
                },
            ],
            new: |d, sr| Box::new(get_new_df(1)(d, sr)),
        },
        1 => PluginDescriptor {
            unique_id: ID_STEREO,
            label: "deep_filter_stereo",
            properties: ladspa::Properties::PROP_NONE,
            name: "DeepFilter Stereo",
            maker: "Hendrik Schröter",
            copyright: "MIT/Apache",
            ports: vec![
                Port {
                    name: "Audio In L",
                    desc: PortDescriptor::AudioInput,
                    ..Default::default()
                },
                Port {
                    name: "Audio In R",
                    desc: PortDescriptor::AudioInput,
                    ..Default::default()
                },
                Port {
                    name: "Audio Out L",
                    desc: PortDescriptor::AudioOutput,
                    ..Default::default()
                },
                Port {
                    name: "Audio Out R",
                    desc: PortDescriptor::AudioOutput,
                    ..Default::default()
                },
                Port {
                    name: "Attenuation Limit (dB)",
                    desc: PortDescriptor::ControlInput,
                    hint: None,
                    default: Some(ATTEN_LIM_DEF),
                    lower_bound: Some(ATTEN_LIM_MIN),
                    upper_bound: Some(ATTEN_LIM_MAX),
                },
                Port {
                    name: "Min processing threshold (dB)",
                    desc: PortDescriptor::ControlInput,
                    hint: None,
                    default: Some(MIN_PROC_THRESH_DEF),
                    lower_bound: Some(MIN_PROC_THRESH_MIN),
                    upper_bound: Some(MIN_PROC_THRESH_MAX),
                },
                Port {
                    name: "Max ERB processing threshold (dB)",
                    desc: PortDescriptor::ControlInput,
                    hint: None,
                    default: Some(MAX_ERB_BUF_DEF),
                    lower_bound: Some(MAX_ERB_BUF_MIN),
                    upper_bound: Some(MAX_ERB_BUF_MAX),
                },
                Port {
                    name: "Max DF processing threshold (dB)",
                    desc: PortDescriptor::ControlInput,
                    hint: None,
                    default: Some(MAX_DF_BUF_DEF),
                    lower_bound: Some(MAX_DF_BUF_MIN),
                    upper_bound: Some(MAX_DF_BUF_MAX),
                },
                Port {
                    name: "Min Processing Buffer (frames)",
                    desc: PortDescriptor::ControlInput,
                    hint: None,
                    default: Some(MIN_PROC_BUF_DEF),
                    lower_bound: Some(MIN_PROC_BUF_MIN),
                    upper_bound: Some(MIN_PROC_BUF_MAX),
                },
                Port {
                    name: "Post Filter Beta",
                    desc: PortDescriptor::ControlInput,
                    hint: None,
                    default: Some(PF_BETA_DEF),
                    lower_bound: Some(PF_BETA_MIN),
                    upper_bound: Some(PF_BETA_MAX),
                },
            ],
            new: |d, sr| Box::new(get_new_df(2)(d, sr)),
        },
        _ => {
            log::error!("Unexpected plugin index: {index}");
            return None;
        }
    };
    Some(descriptor)
}
