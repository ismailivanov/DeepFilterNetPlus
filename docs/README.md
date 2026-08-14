# DeepFilterNet Plus Documentation

Welcome! Choose the guide that matches what you want to do.

## For users

| Guide | Use it when… |
| --- | --- |
| [Quick start](../README.md) | You want the shortest route from download to working noise suppression |
| [EasyEffects setup](EASYEFFECTS.md) | You want a cleaner Linux microphone for calls, streaming, or recording |
| [Command-line app](CLI.md) | You want to clean recorded WAV files on Linux, macOS, or Windows |
| [PipeWire virtual microphone](../ladspa/README.md) | You want a Linux virtual microphone without EasyEffects |

## For developers and researchers

The [developer and research guide](DEVELOPER_GUIDE.md) covers:

- the Rust and Python project structure;
- Python and manual installation;
- command-line and library usage;
- dataset preparation and model training;
- papers and citation information.

Active engineering investigations:

- [LADSPA overload recovery report](LADSPA_OVERLOAD_REPORT.md) — field evidence,
  source analysis, and a regression-test plan for real-time callback stalls.

## Downloads

- [Latest DeepFilterNet Plus release](https://github.com/ismailivanov/DeepFilterNetPlus/releases/latest)
- [EasyEffects on Flathub](https://flathub.org/apps/com.github.wwmm.easyeffects)

If something does not work, open an
[issue](https://github.com/ismailivanov/DeepFilterNetPlus/issues) and include your operating system,
installation method, and the exact error message.
