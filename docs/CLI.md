# Command-line App

The `deep-filter` command cleans background noise from recorded WAV files. Prebuilt executables
include the default model, so you do not need Python, Rust, or a separate model download.

## Download

| Platform | File |
| --- | --- |
| Linux x86_64 | [`deep-filter-linux-x86_64`](https://github.com/ismailivanov/DeepFilterNetPlus/releases/latest/download/deep-filter-linux-x86_64) |
| macOS — Apple Silicon | [`deep-filter-macos-arm64`](https://github.com/ismailivanov/DeepFilterNetPlus/releases/latest/download/deep-filter-macos-arm64) |
| macOS — Intel | [`deep-filter-macos-x86_64`](https://github.com/ismailivanov/DeepFilterNetPlus/releases/latest/download/deep-filter-macos-x86_64) |
| Windows x86_64 | [`deep-filter-windows-x86_64.exe`](https://github.com/ismailivanov/DeepFilterNetPlus/releases/latest/download/deep-filter-windows-x86_64.exe) |

## Linux

```bash
chmod +x deep-filter-linux-x86_64
./deep-filter-linux-x86_64 noisy.wav --output-dir cleaned/
```

## macOS

Use the filename that matches your Mac:

```bash
chmod +x deep-filter-macos-arm64
./deep-filter-macos-arm64 noisy.wav --output-dir cleaned/
```

If Gatekeeper blocks the downloaded file, remove the quarantine attribute once:

```bash
xattr -d com.apple.quarantine deep-filter-macos-arm64
```

For an Intel Mac, replace `deep-filter-macos-arm64` with `deep-filter-macos-x86_64`.

## Windows

Open PowerShell in the folder containing the downloaded file:

```powershell
.\deep-filter-windows-x86_64.exe noisy.wav --output-dir cleaned
```

## Useful options

```text
-o, --output-dir <FOLDER>   Choose the output folder
-D, --compensate-delay     Remove processing delay from the output
    --pf                    Enable stronger post-filtering
-m, --model <FILE>         Use a different model archive
-v                         Show more log information
-h, --help                 Show every available option
```

You can process multiple files in one command:

```bash
./deep-filter-linux-x86_64 interview-1.wav interview-2.wav --output-dir cleaned/
```

The output keeps each input filename and writes the cleaned files into the selected folder.
