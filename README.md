# turing-rs

Standalone system monitor for the Turing Smart Screen 3.5" (rev A) — a single binary, no Python.
Protocol and theme format ported from [turing-smart-screen-python](https://github.com/mathoudebine/turing-smart-screen-python).

- CPU %, temp, clock · GPU (NVIDIA/AMD/Intel iGPU on Linux, LibreHardwareMonitor on Windows) · RAM · disk · network · date/time
- Built-in landscape dashboard, or any `theme.yaml` from the Python project
- Only changed screen regions are sent, so updates don't visibly sweep
- System tray (Linux SNI + Windows): switch theme, brightness, open config, reload, quit
- Hot reload: edit `config.toml` or any file in the active theme folder and the screen updates within a second
- Linux and Windows; prebuilt binaries on the [Releases](https://github.com/n-delic/turing-rs/releases) page (built by GitHub Actions)

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

## Tray

The app lives in the system tray. *Theme* lists every folder with a `theme.yaml` under `~/.config/turing-rs/themes/` (or `%APPDATA%\turing-rs\themes\`) and the `themes/` folder next to the binary. Picking one, or a brightness, is written back to `config.toml`.

## Themes

```toml
theme = "~/.config/turing-rs/themes/CyberpunkLandscape"   # folder with theme.yaml
# fonts_dir = "~/.config/turing-rs/fonts"                  # the Python repo's res/fonts
```

`themes/CyberpunkLandscape` is a landscape remake of the Python project's Cyberpunk theme. Other themes: copy a folder from the Python repo's `res/themes/` and the fonts it uses from `res/fonts/`. Supported: static images/text, TEXT, GRAPH and RADIAL elements for CPU, GPU, MEMORY, DISK, NET, DATE, UPTIME. Not yet: LINE_GRAPH, WEATHER, PING, CUSTOM.

Set `TURING_PNG=frame.png` to also write each frame to disk while tweaking a theme.

## Windows build

Push a tag like `v0.3.0` and the `build` workflow attaches `turing-rs-windows-x86_64.exe` and the Linux binary to a GitHub release; every push to `main` also uploads them as workflow artifacts.

## License

GPL-3.0-or-later, same as the project it is ported from.
