# turing-rs

Standalone system monitor for the Turing Smart Screen 3.5" (rev A) — a single binary, no Python.
Protocol and theme format ported from [turing-smart-screen-python](https://github.com/mathoudebine/turing-smart-screen-python).

- CPU %, temp, clock · GPU (NVIDIA/AMD/Intel iGPU on Linux, LibreHardwareMonitor on Windows) · RAM · disk · network · date/time
- Built-in landscape dashboard, or any `theme.yaml` from the Python project
- Only changed screen regions are sent, so updates don't visibly sweep
- Linux and Windows

## Build

```
cargo build --release          # target/release/turing-rs (~7 MB, no runtime deps)
```

Linux: your user needs access to the serial port (`sudo usermod -aG uucp $USER` on Arch, `dialout` on Debian/Ubuntu), then re-login.

## Run

```
./turing-rs                    # auto-detects the screen, uses the built-in dashboard
./turing-rs path/to/config.toml
```

Config lives at `~/.config/turing-rs/config.toml` (Linux) or `%APPDATA%\turing-rs\config.toml` (Windows); see `config.example.toml`.

Start at login on Linux: `cp turing-rs.service ~/.config/systemd/user/ && systemctl --user enable --now turing-rs` (expects the binary in `~/.local/bin`).

Windows: run LibreHardwareMonitor with *Options → Remote Web Server* enabled; CPU temp and GPU stats are read from it.

## Themes

```toml
theme = "~/.config/turing-rs/themes/CyberpunkLandscape"   # folder with theme.yaml
# fonts_dir = "~/.config/turing-rs/fonts"                  # the Python repo's res/fonts
```

`themes/CyberpunkLandscape` is a landscape remake of the Python project's Cyberpunk theme. Other themes: copy a folder from the Python repo's `res/themes/` and the fonts it uses from `res/fonts/`. Supported: static images/text, TEXT, GRAPH and RADIAL elements for CPU, GPU, MEMORY, DISK, NET, DATE, UPTIME. Not yet: LINE_GRAPH, WEATHER, PING, CUSTOM.

Set `TURING_PNG=frame.png` to also write each frame to disk while tweaking a theme.

## License

GPL-3.0-or-later, same as the project it is ported from.
