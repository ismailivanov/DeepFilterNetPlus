# EasyEffects Setup

EasyEffects gives DeepFilterNet a graphical interface and makes it easy to use noise suppression
with Discord, OBS, browsers, games, and other Linux applications.

## Choose an installation

### Option A — EasyEffects Flatpak

[Install EasyEffects from Flathub](https://flathub.org/apps/com.github.wwmm.easyeffects) if you want
the fastest setup. The Flatpak includes the dependencies for **Deep Noise Remover**, so you do not
need to download a separate plugin.

```bash
flatpak install flathub com.github.wwmm.easyeffects
```

Flatpak uses its own bundled plugins. To use the DeepFilterNet Plus build from this repository
specifically, use Option B.

### Option B — native EasyEffects + DeepFilterNet Plus

1. Install the native `easyeffects` package with your Linux distribution's package manager.
2. Download
   [`libdeep_filter_ladspa.so`](https://github.com/ismailivanov/DeepFilterNetPlus/releases/latest/download/libdeep_filter_ladspa.so).
3. Install the plugin:

   ```bash
   sudo install -Dm755 ~/Downloads/libdeep_filter_ladspa.so \
     /usr/lib/ladspa/libdeep_filter_ladspa.so
   ```

4. Fully quit and reopen EasyEffects so it scans the plugin again.

Some distributions use `/usr/lib64/ladspa/` instead. If **Deep Noise Remover** is still unavailable,
check which LADSPA directory your distribution uses.

## Add noise suppression to your microphone

1. Open EasyEffects.
2. Select **Input**.
3. Make sure your physical microphone is selected.
4. Add **Deep Noise Remover** to the effects list.
5. Speak and confirm that the input level moves.
6. In the application where you need the processed audio, select **Easy Effects Source** as the
   microphone.

Keep your physical microphone as the system default device; select **Easy Effects Source** only
inside the applications that should receive processed audio.

## Update DeepFilterNet Plus

Download the newest `libdeep_filter_ladspa.so`, run the same `install` command again, and restart
EasyEffects. Your existing EasyEffects presets remain in place.

## Troubleshooting

### Deep Noise Remover is missing

- Confirm that you restarted EasyEffects completely.
- Confirm that the file exists in `/usr/lib/ladspa/` or your distribution's LADSPA directory.
- If you installed EasyEffects through Flatpak, use its bundled Deep Noise Remover instead of a
  plugin copied to the host system.

### Other apps still hear the unprocessed microphone

Open that application's audio settings and change its microphone from the physical device to
**Easy Effects Source**.

### Crackling or dropouts

Confirm that the Plus version of `libdeep_filter_ladspa.so` replaced the older native plugin. If
the problem continues, open an
[issue](https://github.com/ismailivanov/DeepFilterNetPlus/issues) with your PipeWire quantum,
EasyEffects installation method, CPU model, and EasyEffects logs.
