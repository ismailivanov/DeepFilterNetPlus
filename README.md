<div align="center">

# DeepFilterNet Plus

Fast, AI-powered noise suppression for microphones and recorded audio.

[![Download DeepFilterNet Plus](https://img.shields.io/badge/Download-DeepFilterNet_Plus-2ea44f?style=for-the-badge&logo=github)](https://github.com/ismailivanov/DeepFilterNetPlus/releases/latest)
[![Get EasyEffects](https://img.shields.io/badge/Get-EasyEffects-3584e4?style=for-the-badge&logo=flathub&logoColor=white)](https://flathub.org/apps/com.github.wwmm.easyeffects)
[![Documentation](https://img.shields.io/badge/Open-Documentation-6f42c1?style=for-the-badge&logo=readthedocs&logoColor=white)](docs/README.md)
[![Buy Me a Coffee](https://img.shields.io/badge/Buy_Me_a_Coffee-Support-FFDD00?style=for-the-badge&logo=buy-me-a-coffee&logoColor=000000)](https://buymeacoffee.com/carbon06)

</div>

DeepFilterNet Plus is a maintained fork of
[DeepFilterNet](https://github.com/Rikorose/DeepFilterNet). It removes background noise from a
live microphone on Linux and cleans up recorded WAV files on Linux, macOS, and Windows.

No compilation is required. Pick what you want to do:

| I want to… | Recommended option |
| --- | --- |
| Clean my microphone on Linux | [EasyEffects + Deep Noise Remover](#microphone-noise-suppression-linux) |
| Clean a recorded audio file | [Download the command-line app](#recorded-audio-linux-macos-windows) |
| Build, train, or use the Python package | [Developer and research guide](docs/DEVELOPER_GUIDE.md) |

## Microphone noise suppression (Linux)

### Easiest setup

1. [Install EasyEffects from Flathub](https://flathub.org/apps/com.github.wwmm.easyeffects).
2. Open EasyEffects and select **Input**.
3. Add **Deep Noise Remover** to the effects list.
4. Select **Easy Effects Source** as the microphone in Discord, OBS, your browser, or another app.

The EasyEffects Flatpak already includes the components needed for **Deep Noise Remover**.

> Want the real-time safety and stereo fixes from this Plus fork specifically? Install the native
> EasyEffects package from your Linux distribution, then follow the
> [DeepFilterNet Plus plugin guide](docs/EASYEFFECTS.md).

## Recorded audio (Linux, macOS, Windows)

Download the ready-to-run file for your computer:

| Platform | Download |
| --- | --- |
| Linux x86_64 | [`deep-filter-linux-x86_64`](https://github.com/ismailivanov/DeepFilterNetPlus/releases/latest/download/deep-filter-linux-x86_64) |
| macOS — Apple Silicon | [`deep-filter-macos-arm64`](https://github.com/ismailivanov/DeepFilterNetPlus/releases/latest/download/deep-filter-macos-arm64) |
| macOS — Intel | [`deep-filter-macos-x86_64`](https://github.com/ismailivanov/DeepFilterNetPlus/releases/latest/download/deep-filter-macos-x86_64) |
| Windows x86_64 | [`deep-filter-windows-x86_64.exe`](https://github.com/ismailivanov/DeepFilterNetPlus/releases/latest/download/deep-filter-windows-x86_64.exe) |

On Linux (use the matching downloaded filename on macOS):

```bash
chmod +x deep-filter-linux-x86_64
./deep-filter-linux-x86_64 noisy.wav --output-dir cleaned/
```

On macOS, Gatekeeper may block the downloaded app the first time. Give it permission with:

```bash
chmod +x deep-filter-macos-arm64
xattr -d com.apple.quarantine deep-filter-macos-arm64
```

On an Intel Mac, replace `deep-filter-macos-arm64` with `deep-filter-macos-x86_64`.

On Windows PowerShell:

```powershell
.\deep-filter-windows-x86_64.exe noisy.wav --output-dir cleaned
```

The cleaned WAV file is written to the output folder. See the
[command-line guide](docs/CLI.md) for macOS Gatekeeper help, extra options, and more examples.

## Why the Plus fork?

- **Real-time safe LADSPA processing:** the audio callback no longer blocks, preventing
  system-wide crackling at small PipeWire buffer sizes.
- **Safer overload handling:** audio passes through instead of taking down the host if the worker
  cannot keep up.
- **Stereo inference fix:** current `tract` versions work correctly with multi-channel models.
- **Drop-in compatibility:** the library name and plugin identifiers remain unchanged for
  EasyEffects and PipeWire filter-chain setups.

See [what changed from upstream](docs/DEVELOPER_GUIDE.md#plus-fork-changes) for implementation
details.

## Documentation

- [EasyEffects setup](docs/EASYEFFECTS.md)
- [Command-line app](docs/CLI.md)
- [PipeWire virtual microphone](ladspa/README.md)
- [Developer, Python, training, and research guide](docs/DEVELOPER_GUIDE.md)
- [All documentation](docs/README.md)

## Support the project

If DeepFilterNet Plus makes your calls or recordings better, you can support continued maintenance:

[![Buy Me a Coffee](https://img.shields.io/badge/Buy_Me_a_Coffee-carbon06-FFDD00?style=for-the-badge&logo=buy-me-a-coffee&logoColor=000000)](https://buymeacoffee.com/carbon06)

## Credits and license

DeepFilterNet Plus builds on the work of the
[DeepFilterNet contributors](https://github.com/Rikorose/DeepFilterNet/graphs/contributors).
The project is available under your choice of the [MIT](LICENSE-MIT) or
[Apache 2.0](LICENSE-APACHE) license.
