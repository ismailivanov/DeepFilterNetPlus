#!/usr/bin/env python3
"""Smoke test for the LADSPA plugin's overload recovery path.

Drives target/release/libdeep_filter_ladspa.so directly through the LADSPA
C ABI (no host needed) and checks, in order:

  A) real-time paced feed   -> audio is processed (noise RMS drops, not silence)
  B) faster-than-real-time  -> sustained overload; the circuit breaker must
                               engage bypass (output == input, bit-exact)
  C) after the 5s cooldown  -> processing resumes (output != input, RMS drops)

and that the total log output stays bounded (a handful of lines, not ~50/s
as before the fix; see docs/LADSPA_OVERLOAD_REPORT.md).

Usage: python ladspa/overload_smoke.py   (after cargo build --release)
"""
import array
import ctypes
import math
import os
import sys
import tempfile
import time
import wave

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SO = os.path.join(ROOT, "target", "release", "libdeep_filter_ladspa.so")
WAV = os.path.join(ROOT, "assets", "noisy_snr0.wav")
SR = 48000
Q = 1024  # host quantum, matches the field incident

# Defaults matching DfControlHistory so no control messages are sent.
CONTROLS = {
    b"Attenuation Limit (dB)": 100.0,
    b"Post Filter Beta": 0.0,
    b"Min processing threshold (dB)": -10.0,
    b"Max ERB processing threshold (dB)": 30.0,
    b"Max DF processing threshold (dB)": 20.0,
    b"Min Processing Buffer (frames)": 0.0,
}

PORT_INPUT, PORT_OUTPUT, PORT_CONTROL, PORT_AUDIO = 1, 2, 4, 8

LData = ctypes.c_float
Handle = ctypes.c_void_p


class Descriptor(ctypes.Structure):
    pass


Descriptor._fields_ = [
    ("UniqueID", ctypes.c_ulong),
    ("Label", ctypes.c_char_p),
    ("Properties", ctypes.c_int),
    ("Name", ctypes.c_char_p),
    ("Maker", ctypes.c_char_p),
    ("Copyright", ctypes.c_char_p),
    ("PortCount", ctypes.c_ulong),
    ("PortDescriptors", ctypes.POINTER(ctypes.c_int)),
    ("PortNames", ctypes.POINTER(ctypes.c_char_p)),
    ("PortRangeHints", ctypes.c_void_p),
    ("ImplementationData", ctypes.c_void_p),
    ("instantiate", ctypes.CFUNCTYPE(Handle, ctypes.POINTER(Descriptor), ctypes.c_ulong)),
    ("connect_port", ctypes.CFUNCTYPE(None, Handle, ctypes.c_ulong, ctypes.POINTER(LData))),
    ("activate", ctypes.CFUNCTYPE(None, Handle)),
    ("run", ctypes.CFUNCTYPE(None, Handle, ctypes.c_ulong)),
    ("run_adding", ctypes.c_void_p),
    ("set_run_adding_gain", ctypes.c_void_p),
    ("deactivate", ctypes.c_void_p),
    ("cleanup", ctypes.CFUNCTYPE(None, Handle)),
]


def rms(buf):
    a = array.array("f")
    a.frombytes(bytes(buf))
    return math.sqrt(sum(x * x for x in a) / len(a))


def main():
    os.environ["RUST_LOG"] = "info"
    lib = ctypes.CDLL(SO)
    lib.ladspa_descriptor.restype = ctypes.POINTER(Descriptor)

    # Find the mono descriptor (1 audio in, 1 audio out).
    desc = None
    for idx in range(4):
        d = lib.ladspa_descriptor(idx)
        if not d:
            break
        pd = d.contents.PortDescriptors
        n_in = sum(
            1
            for p in range(d.contents.PortCount)
            if pd[p] & PORT_AUDIO and pd[p] & PORT_INPUT
        )
        if n_in == 1:
            desc = d.contents
            break
    assert desc is not None, "mono descriptor not found"

    w = wave.open(WAV)
    assert (w.getnchannels(), w.getframerate(), w.getsampwidth()) == (1, SR, 2), "unexpected wav format"
    pcm = array.array("h")
    pcm.frombytes(w.readframes(w.getnframes()))
    fl = array.array("f", (s / 32768.0 for s in pcm))
    quanta = [fl[i * Q:(i + 1) * Q].tobytes() for i in range(len(fl) // Q)]

    handle = desc.instantiate(ctypes.byref(desc), SR)
    assert handle, "instantiate failed"

    in_buf = (LData * Q)()
    out_buf = (LData * Q)()
    control_refs = []  # keep alive
    for p in range(desc.PortCount):
        pd = desc.PortDescriptors[p]
        name = desc.PortNames[p]
        if pd & PORT_AUDIO:
            desc.connect_port(handle, p, in_buf if pd & PORT_INPUT else out_buf)
        else:
            assert name in CONTROLS, f"unknown control port {name}"
            c = LData(CONTROLS[name])
            control_refs.append(c)
            desc.connect_port(handle, p, ctypes.byref(c))

    def run_quantum(data):
        ctypes.memmove(in_buf, data, Q * 4)
        desc.run(handle, Q)
        return bytes(out_buf)

    # --- Phase A: real-time paced, expect processed audio -----------------
    in_rms = out_rms = 0.0
    for i, data in enumerate(quanta[:250]):
        out = run_quantum(data)
        if i >= 50:  # skip warmup / initial latency
            in_rms += rms(data)
            out_rms += rms(out)
        time.sleep(Q / SR)
    assert out_rms > 0.02 * in_rms, f"output is silence (out={out_rms:.4f} in={in_rms:.4f})"
    assert out_rms < 0.9 * in_rms, f"no noise reduction (out={out_rms:.4f} in={in_rms:.4f})"
    print(f"A: processed OK, rms out/in = {out_rms / in_rms:.3f}")

    # --- Phase B: overload (feed much faster than real time) --------------
    bypassed_at = None
    for i in range(3000):
        data = quanta[i % len(quanta)]
        if run_quantum(data) == data:
            bypassed_at = i
            break
    assert bypassed_at is not None, "circuit breaker never engaged bypass"
    print(f"B: bypass engaged after {bypassed_at} overloaded quanta")

    # --- Phase C: recovery after cooldown ---------------------------------
    # Bypass counts down in samples; feed paced until past 5s + margin.
    for i in range(280):
        run_quantum(quanta[i % len(quanta)])
        time.sleep(Q / SR)
    in_rms = out_rms = exact = 0
    for i in range(100):
        data = quanta[i % len(quanta)]
        out = run_quantum(data)
        if i >= 50:
            in_rms += rms(data)
            out_rms += rms(out)
            exact += out == data
        time.sleep(Q / SR)
    assert exact == 0, "still bypassed after cooldown"
    assert 0.02 * in_rms < out_rms < 0.9 * in_rms, f"processing did not resume (out={out_rms:.4f} in={in_rms:.4f})"
    print(f"C: processing resumed, rms out/in = {out_rms / in_rms:.3f}")


if __name__ == "__main__":
    # Capture the plugin's stderr logging so it can be counted.
    log = tempfile.TemporaryFile(mode="w+")
    old_stderr = os.dup(2)
    os.dup2(log.fileno(), 2)
    try:
        main()
    finally:
        os.dup2(old_stderr, 2)
    log.seek(0)
    lines = log.read().splitlines()
    print(f"log lines total: {len(lines)}")
    for line in lines:
        print(f"  | {line}")
    assert len(lines) <= 20, f"log storm: {len(lines)} lines"
    print("PASS")
