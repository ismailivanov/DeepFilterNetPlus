# LADSPA overload recovery can amplify system stalls

Status: investigation report; reproduced in field logs, not yet reproduced by an automated test.

## Summary

Under sustained host or system pressure, the LADSPA plugin repeatedly grows its output delay to about one second, drops both queues, resets to one frame, and immediately starts the same cycle again. Each step logs from the real-time callback. Two CachyOS/EasyEffects incidents ended in an unresponsive KDE Wayland session and a forced reboot.

The evidence does **not** show that DeepFilterNet Plus initiated the original system-wide pressure. It does show that the current overload path can create an audio-thread log/recovery storm and plausibly amplify an existing stall.

Proposed issue title:

> LADSPA overload recovery causes a real-time log storm and repeated backlog-reset loop under system pressure

## Affected build and environment

- Installed package: `deepfilternet-plus-bin 1.0.1-1`
- Installed plugin: `/usr/lib/ladspa/libdeep_filter_ladspa.so`
- Installed plugin SHA-256: `851d3fc8622a40d2e779499a177b1c0afb25c7fe93d4e84b6c64868aa990bd82`
- Repository revision inspected: `c379acf323d3ac7dac26b66a61b13c3a52457502`
- Host: EasyEffects 8.2.8, PipeWire 1.6.8, WirePlumber 0.5.15
- Audio format: 48 kHz, PipeWire quantum 1024
- System: CachyOS, Linux `7.1.5-1-cachyos`, KDE Plasma Wayland, 16 GiB RAM and 15.3 GiB zram

The installed binary hash does not match the current local release artifacts. Any reproduction or fix validation must therefore record the exact binary under test.

## Field evidence

All times below are local CEST journal timestamps. The timestamp embedded in the EasyEffects message is UTC.

| Incident window | Output underruns | Backlog resets | PipeWire `Broken pipe` | KWin main-thread stalls | SysRq rejected |
| --- | ---: | ---: | ---: | ---: | ---: |
| 2026-07-31 21:02:45–21:07:56 | 3,302 | 70 | 3 | 4 | 0 |
| 2026-07-31 23:34:00–23:39:42 | 2,310 | 49 | 1 | 2 | 11 |

Both boots ended abruptly. No NVIDIA Xid/GPU reset, NVMe/Btrfs I/O error, watchdog lockup, or thermal-shutdown marker was found in either final incident window. During the second incident, the kernel continued recording SysRq attempts and Docker messages, which points to a severely stalled user session rather than a completely dead kernel.

The first investigation also captured an earlier system OOM event that killed an Electron process when only 216 KiB of zram swap remained. That establishes pre-existing resource pressure, but it does not establish DeepFilterNet Plus as the source of the memory exhaustion.

Typical repeating sequence:

```text
Output underrun. Increasing processing latency to 31.3ms
...
Output underrun. Increasing processing latency to 1012.7ms
Processing too slow, dropping backlog. Try to decrease 'Max DF processing threshold (dB)'.
Output underrun. Increasing processing latency to 31.3ms
```

At 48 kHz with 1024-sample frames, one frame is about 21.3 ms. The observed staircase and one-second reset match the implementation.

## Relevant implementation

The affected path is `ladspa/src/lib.rs`, primarily `DfPlugin::run()`:

- Lines 430–433 update control history and use a bounded `SyncSender::send()`. If its 32-entry channel is full, the real-time callback can block.
- Lines 436–441 lock the input `Mutex<Vec<VecDeque<f32>>>` and may grow queues while copying samples.
- Lines 448–466 lock and drain the output queues.
- Lines 468–501 clear or grow queues and log directly from the callback for every overload step.
- Lines 513–527 perform further queue mutation and direct informational logging during latency reduction.

The current code already avoids waiting for inference completion and passes audio through if the worker dies. Those protections do not cover mutex contention, bounded control-channel blocking, allocation, or the repeated reset/log loop.

## Working hypothesis

1. The worker misses its deadline because of CPU scheduling delay, memory reclaim, or inference time.
2. The callback emits silence and grows `proc_delay` by at least one model frame.
3. Every growth step logs synchronously from the callback.
4. At one second of delay, both queues are cleared and the delay resets to one frame.
5. If the worker is still overloaded, the same cycle immediately repeats.
6. Queue locks, allocation, synchronous logging, and possible control-channel blocking add more work or contention to the real-time path.

This is a resilience bug even if the initial overload originates elsewhere: recovery should converge to a bounded degraded mode rather than repeatedly rebuilding and discarding one second of latency.

## Recommended implementation direction

1. Replace `control_tx.send()` in the callback with a non-blocking update path. Coalescing controls by key is preferable to queueing every intermediate value.
2. Move warning/error emission out of the callback. Increment atomics or enqueue compact events, then aggregate and rate-limit logs on a non-RT thread.
3. Replace shared `Mutex<VecDeque>` queues with bounded, preallocated SPSC buffers so the callback performs no allocation and never waits for the worker.
4. Add an overload circuit breaker. After sustained overload, enter bypass or bounded-silence mode for a cooldown interval instead of immediately restarting the one-second staircase.
5. Tag input/output with a generation number when clearing a backlog so stale worker output from the previous generation cannot re-enter the new stream.
6. Expose counters for underruns, backlog resets, maximum queue depth, and time spent in degraded mode.

## Regression test plan

Add a deterministic test worker whose processing time can be delayed independently of the audio callback.

Test cases:

- worker slower than real time for 5–30 seconds;
- worker temporarily paused while controls change more than 32 times;
- callback and worker contending for input/output queues;
- recovery after the worker returns below real-time factor 1.0;
- stale output arriving after a backlog reset;
- small host quanta such as 32/64 and the observed quantum 1024.

Acceptance criteria:

- the callback never blocks on a channel or mutex;
- no allocation or synchronous logging occurs in the callback;
- logs remain bounded, for example one summary per overload episode;
- queue memory remains bounded;
- overload reaches a stable bypass/degraded state instead of cycling every second;
- normal processed audio resumes without replaying stale output;
- the PipeWire graph remains responsive throughout the injected stall.

## Useful journal queries

```bash
journalctl -b -1 --no-pager | rg 'Output underrun|Processing too slow|Broken pipe|main thread was hanging'

journalctl -b -1 --since '2026-07-31 23:34:00' --until '2026-07-31 23:39:42' --no-pager
```

## Confidence and limitations

High confidence:

- the repeating latency/reset sequence is produced by the inspected source path;
- the log volume and downstream audio/KWin stalls occurred in the same windows;
- the callback currently contains operations unsuitable for a hard real-time path.

Not yet proven:

- which individual operation contributes most to the system-wide stall;
- whether the current repository build behaves identically to the installed binary;
- whether removing the log storm alone is sufficient;
- whether DeepFilterNet Plus initiates resource pressure rather than amplifying it.

No fix should be considered complete until the synthetic overload test passes and a matching plugin binary survives an EasyEffects/PipeWire reproduction without making the desktop unresponsive.
