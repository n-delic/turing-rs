// Turing Smart Screen 3.5" (rev A) system monitor.
// Protocol ported from turing-smart-screen-python (GPL-3.0).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod sensors;
mod theme;
mod tray;
mod gui;

use ab_glyph::{FontRef, PxScale};
use image::{Rgb, RgbImage};
use imageproc::drawing::{draw_filled_rect_mut, draw_text_mut, text_size};
use imageproc::rect::Rect;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::{thread, time::Duration};

pub const MONO: &[u8] = include_bytes!("../assets/LiberationMono-Bold.ttf");
const SANS: &[u8] = include_bytes!("../assets/LiberationSans-Bold.ttf");

#[derive(Deserialize, Serialize)]
#[serde(default)]
struct Config {
    #[serde(skip_serializing_if = "Option::is_none")]
    port: Option<String>,
    brightness: u8,
    interval_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    net_interface: Option<String>,
    disk: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    lhm_url: Option<String>,
    accent: String,
    /// Directory containing a turing-smart-screen-python theme.yaml. Unset = built-in dashboard.
    #[serde(skip_serializing_if = "Option::is_none")]
    theme: Option<PathBuf>,
    /// Python project's res/fonts dir. Default: <theme>/../../fonts
    #[serde(skip_serializing_if = "Option::is_none")]
    fonts_dir: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: None,
            brightness: 50,
            interval_ms: 1000,
            net_interface: None,
            disk: if cfg!(windows) { "C:\\".into() } else { "/".into() },
            lhm_url: if cfg!(windows) { Some("http://localhost:8085/data.json".into()) } else { None },
            accent: "#4fc3f7".into(),
            theme: None,
            fonts_dir: None,
        }
    }
}

/// State shared between the render loop and the settings window.
#[derive(Default)]
pub struct Shared {
    pub frame: Option<RgbImage>,
    pub seq: u64,
    pub show: bool,
    pub ctx: Option<eframe::egui::Context>,
}

fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_default().join("turing-rs")
}

fn config_path() -> PathBuf {
    std::env::args().skip(1).find(|a| !a.starts_with("--")).map(Into::into).unwrap_or_else(|| config_dir().join("config.toml"))
}

fn load_config() -> Config {
    let mut cfg: Config = match std::fs::read_to_string(config_path()) {
        Ok(s) => toml::from_str(&s).unwrap_or_else(|e| {
            eprintln!("bad config ({e}), using defaults");
            Config::default()
        }),
        Err(_) => Config::default(),
    };
    for p in [&mut cfg.theme, &mut cfg.fonts_dir].into_iter().flatten() {
        if let (Ok(rest), Some(home)) = (p.strip_prefix("~"), dirs::home_dir()) {
            *p = home.join(rest);
        }
    }
    cfg
}

/// ponytail: rewrites the whole file, so comments in config.toml are lost on first tray change.
fn save_config(cfg: &Config) {
    let path = config_path();
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")));
    if let Err(e) = std::fs::write(&path, toml::to_string_pretty(cfg).unwrap()) {
        eprintln!("cannot save {}: {e}", path.display());
    }
}

/// Theme folders under <config>/themes and the repo's themes/ next to the binary.
fn list_themes() -> Vec<(String, PathBuf)> {
    let mut roots = vec![config_dir().join("themes")];
    if let Ok(exe) = std::env::current_exe() {
        roots.extend(exe.ancestors().take(4).map(|a| a.join("themes")));
    }
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    for root in roots {
        for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
            let p = e.path();
            if p.join("theme.yaml").is_file() {
                let name = e.file_name().to_string_lossy().into_owned();
                if !out.iter().any(|(n, _)| *n == name) {
                    out.push((name, p));
                }
            }
        }
    }
    out.sort();
    out
}

/// Newest mtime across the config file and the theme folder — polled to hot-reload edits.
fn stamp(theme: Option<&Path>) -> u128 {
    let mt = |p: &Path| p.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis());
    let mut s = mt(&config_path());
    if let Some(dir) = theme {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            s = s.max(mt(&e.path()));
        }
    }
    s
}

fn open_folder(p: &Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(p).spawn();
    #[cfg(not(windows))]
    let _ = std::process::Command::new("xdg-open").arg(p).spawn();
}

fn hex(s: &str) -> Rgb<u8> {
    let v = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap_or(0x4fc3f7);
    Rgb([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

// ---------- screen protocol ----------

fn cmd(x: u16, y: u16, ex: u16, ey: u16, c: u8) -> [u8; 6] {
    [
        (x >> 2) as u8,
        (((x & 3) << 6) | (y >> 4)) as u8,
        (((y & 15) << 4) | (ex >> 6)) as u8,
        (((ex & 63) << 2) | (ey >> 8)) as u8,
        (ey & 255) as u8,
        c,
    ]
}

fn find_port() -> Option<String> {
    serialport::available_ports().ok()?.into_iter().find_map(|p| match p.port_type {
        serialport::SerialPortType::UsbPort(u)
            if u.vid == 0x1a86 && u.pid == 0x5722 || u.serial_number.as_deref() == Some("USB35INCHIPSV2") =>
        {
            Some(p.port_name)
        }
        _ => None,
    })
}

fn open(cfg: &Config, w: u16, h: u16, landscape: bool) -> Option<Box<dyn serialport::SerialPort>> {
    let port = cfg.port.clone().or_else(find_port)?;
    let mut lcd = serialport::new(&port, 115200)
        .timeout(Duration::from_secs(2))
        .flow_control(serialport::FlowControl::Hardware)
        .open()
        .map_err(|e| eprintln!("{port}: {e}"))
        .ok()?;
    eprintln!("using {port}");
    setup(lcd.as_mut(), cfg, w, h, landscape);
    Some(lcd)
}

fn setup(lcd: &mut dyn serialport::SerialPort, cfg: &Config, w: u16, h: u16, landscape: bool) {
    // orientation (portrait 0 / landscape 2) + 100, then width/height big-endian, padded to 16 bytes
    let mut o = [0u8; 16];
    o[5] = 121;
    o[6] = 100 + if landscape { 2 } else { 0 };
    o[7..9].copy_from_slice(&w.to_be_bytes());
    o[9..11].copy_from_slice(&h.to_be_bytes());
    let _ = lcd.write_all(&o);
    // brightness: 0 = brightest, 255 = darkest
    let _ = lcd.write_all(&cmd(255 - (cfg.brightness.min(100) as u16 * 255 / 100), 0, 0, 0, 110));
}

fn load_theme(cfg: &Config) -> Option<theme::Theme> {
    let d = cfg.theme.as_ref()?;
    match theme::Theme::load(d, cfg.fonts_dir.clone()) {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!("theme {}: {e} — using built-in dashboard", d.display());
            None
        }
    }
}

fn rgb565(img: &RgbImage, x0: u32, y0: u32, x1: u32, y1: u32, buf: &mut Vec<u8>) {
    buf.clear();
    for y in y0..=y1 {
        for x in x0..=x1 {
            let [r, g, b] = img.get_pixel(x, y).0;
            let v = ((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3);
            buf.extend_from_slice(&v.to_le_bytes());
        }
    }
}

/// Push only what changed since `prev`: contiguous bands of dirty rows, each cropped to its dirty columns.
/// A full frame is ~300 KB and visibly sweeps down the panel; text updates are a few KB.
fn push(lcd: &mut dyn serialport::SerialPort, img: &RgbImage, prev: Option<&RgbImage>, buf: &mut Vec<u8>) -> std::io::Result<()> {
    let (w, h) = img.dimensions();
    let mut rects: Vec<(u32, u32, u32, u32)> = Vec::new();
    match prev {
        None => rects.push((0, 0, w - 1, h - 1)),
        Some(p) => {
            let mut band: Option<(u32, u32, u32, u32)> = None; // x0, y0, x1, y1
            for y in 0..h {
                let row = &img.as_raw()[(y * w * 3) as usize..((y + 1) * w * 3) as usize];
                let prow = &p.as_raw()[(y * w * 3) as usize..((y + 1) * w * 3) as usize];
                let dirty = row.chunks(3).zip(prow.chunks(3)).enumerate().filter(|(_, (a, b))| a != b).map(|(x, _)| x as u32);
                let (mut x0, mut x1) = (u32::MAX, 0);
                for x in dirty {
                    x0 = x0.min(x);
                    x1 = x;
                }
                match (&mut band, x0 != u32::MAX) {
                    (Some(b), true) => { b.0 = b.0.min(x0); b.2 = b.2.max(x1); b.3 = y; }
                    (None, true) => band = Some((x0, y, x1, y)),
                    (Some(b), false) => { rects.push(*b); band = None; }
                    (None, false) => {}
                }
            }
            rects.extend(band);
        }
    }
    for (x0, y0, x1, y1) in rects {
        rgb565(img, x0, y0, x1, y1, buf);
        lcd.write_all(&cmd(x0 as u16, y0 as u16, x1 as u16, y1 as u16, 197))?;
        lcd.write_all(buf)?;
    }
    Ok(())
}

// ---------- built-in dashboard (landscape 480x320) ----------

const W: u16 = 480;
const H: u16 = 320;
const BG: Rgb<u8> = Rgb([15, 17, 21]);
const CARD: Rgb<u8> = Rgb([26, 29, 36]);
const TRACK: Rgb<u8> = Rgb([42, 46, 56]);
const TEXT: Rgb<u8> = Rgb([230, 230, 230]);
const DIM: Rgb<u8> = Rgb([138, 143, 152]);
const WARN: Rgb<u8> = Rgb([255, 99, 71]);

struct Painter<'a> {
    img: RgbImage,
    mono: FontRef<'a>,
    sans: FontRef<'a>,
    accent: Rgb<u8>,
}

impl Painter<'_> {
    fn rect(&mut self, x: i32, y: i32, w: u32, h: u32, c: Rgb<u8>) {
        if w > 0 && h > 0 {
            draw_filled_rect_mut(&mut self.img, Rect::at(x, y).of_size(w, h), c);
        }
    }
    fn text(&mut self, x: i32, y: i32, size: f32, c: Rgb<u8>, mono: bool, s: &str) {
        let f = if mono { &self.mono } else { &self.sans };
        draw_text_mut(&mut self.img, c, x, y, PxScale::from(size), f, s);
    }
    fn text_right(&mut self, right: i32, y: i32, size: f32, c: Rgb<u8>, mono: bool, s: &str) {
        let f = if mono { &self.mono } else { &self.sans };
        let (w, _) = text_size(PxScale::from(size), f, s);
        self.text(right - w as i32, y, size, c, mono, s);
    }
    /// Card with title, a big value on the right, a bar, and a dim detail line.
    fn card(&mut self, x: i32, y: i32, w: u32, title: &str, pct: f32, value: &str, detail: &str) {
        self.rect(x, y, w, 92, CARD);
        self.text(x + 12, y + 8, 20.0, DIM, false, title);
        let color = if pct >= 85.0 { WARN } else { self.accent };
        self.text_right(x + w as i32 - 12, y + 4, 30.0, TEXT, true, value);
        self.rect(x + 12, y + 44, w - 24, 12, TRACK);
        let fill = ((w - 24) as f32 * pct.clamp(0.0, 100.0) / 100.0) as u32;
        self.rect(x + 12, y + 44, fill, 12, color);
        self.text(x + 12, y + 62, 18.0, DIM, true, detail);
    }
}

fn gb(b: u64) -> String {
    format!("{:.1}G", b as f64 / 1073741824.0)
}

fn rate(bps: f64) -> String {
    if bps >= 1e6 {
        format!("{:>5.1} MB/s", bps / 1e6)
    } else {
        format!("{:>5.0} KB/s", bps / 1e3)
    }
}

fn render(p: &mut Painter, s: &sensors::Stats) {
    p.rect(0, 0, W as u32, H as u32, BG);

    let now = chrono::Local::now();
    p.text(12, 8, 22.0, DIM, false, &now.format("%a %d %b").to_string());
    p.text_right(W as i32 - 12, 2, 34.0, TEXT, true, &now.format("%H:%M:%S").to_string());
    p.rect(12, 42, W as u32 - 24, 2, p.accent);

    let col = (W as u32 - 36) / 2;
    let x2 = 24 + col as i32;

    let temp = s.cpu_temp.map_or("--".into(), |t| format!("{t:.0}°C"));
    p.card(12, 54, col, "CPU", s.cpu, &format!("{:.0}%", s.cpu), &format!("{temp}  {:.2} GHz", s.cpu_mhz as f64 / 1000.0));

    match &s.gpu {
        Some(g) => {
            let temp = g.temp.map_or(String::new(), |t| format!("{t:.0}°C  "));
            let vram = if g.vram_total > 0 { format!("{}/{}", gb(g.vram_used), gb(g.vram_total)) } else { String::new() };
            let mhz = if vram.is_empty() { g.mhz.map_or(String::new(), |m| format!("{m} MHz")) } else { String::new() };
            p.card(x2, 54, col, "GPU", g.load, &format!("{:.0}%", g.load), &format!("{temp}{vram}{mhz}"));
        }
        None => p.card(x2, 54, col, "GPU", 0.0, "--", "no sensor"),
    }

    let ram_pct = s.ram_used as f32 / s.ram_total.max(1) as f32 * 100.0;
    p.card(12, 156, col, "RAM", ram_pct, &format!("{ram_pct:.0}%"), &format!("{} / {}", gb(s.ram_used), gb(s.ram_total)));

    let disk_pct = s.disk_used as f32 / s.disk_total.max(1) as f32 * 100.0;
    p.card(x2, 156, col, "DISK", disk_pct, &format!("{disk_pct:.0}%"), &format!("{} / {}", gb(s.disk_used), gb(s.disk_total)));

    p.rect(12, 258, W as u32 - 24, 50, CARD);
    p.text(24, 266, 20.0, DIM, false, "NET");
    p.text(90, 266, 26.0, p.accent, true, "↓");
    p.text(112, 264, 26.0, TEXT, true, &rate(s.net.rx_bps));
    p.text(290, 266, 26.0, p.accent, true, "↑");
    p.text(312, 264, 26.0, TEXT, true, &rate(s.net.tx_bps));
}

fn main() {
    let shared = Arc::new(Mutex::new(Shared::default()));
    let s2 = shared.clone();
    thread::spawn(move || run_loop(s2));
    // window starts hidden when launched with --tray (e.g. from the systemd unit / autostart)
    gui::run(shared, !std::env::args().any(|a| a == "--tray"));
}

fn run_loop(shared: Arc<Mutex<Shared>>) {
    let mut cfg = load_config();
    let mut theme = load_theme(&cfg);
    let dims = |t: &Option<theme::Theme>| match t {
        Some(t) => (t.width as u16, t.height as u16, t.landscape),
        None => (W, H, true),
    };
    let (mut w, mut h, mut landscape) = dims(&theme);
    let mut lcd = open(&cfg, w, h, landscape);
    if lcd.is_none() {
        eprintln!("screen not found; waiting for it (set `port` in config to force one)");
    }
    let mut sens = sensors::Sensors::new(cfg.net_interface.clone(), cfg.disk.clone(), cfg.lhm_url.clone());
    let mut p = Painter {
        img: RgbImage::new(W as u32, H as u32),
        mono: FontRef::try_from_slice(MONO).unwrap(),
        sans: FontRef::try_from_slice(SANS).unwrap(),
        accent: hex(&cfg.accent),
    };
    let mut buf = Vec::with_capacity(w as usize * h as usize * 2);
    let snapshot = std::env::var("TURING_PNG").ok(); // debug: also write each frame here
    let mut prev: Option<RgbImage> = None;
    let mut last_stamp = stamp(cfg.theme.as_deref());

    let (tx, rx) = mpsc::channel();
    tray::start(list_themes(), cfg.theme.clone(), cfg.brightness, tx);

    loop {
        // tray commands + hot reload when config.toml or the theme folder changes on disk
        let mut reload = false;
        for c in rx.try_iter() {
            match c {
                tray::Cmd::Brightness(b) => {
                    cfg.brightness = b;
                    save_config(&cfg);
                    if let Some(l) = lcd.as_mut() {
                        setup(l.as_mut(), &cfg, w, h, landscape);
                    }
                }
                tray::Cmd::Theme(t) => {
                    cfg.theme = t;
                    save_config(&cfg);
                    reload = true;
                }
                tray::Cmd::OpenConfig => open_folder(&config_dir()),
                tray::Cmd::Settings => {
                    let mut sh = shared.lock().unwrap();
                    sh.show = true;
                    if let Some(c) = &sh.ctx {
                        c.request_repaint();
                    }
                }
                tray::Cmd::Reload => reload = true,
                tray::Cmd::Quit => std::process::exit(0),
            }
        }
        let now_stamp = stamp(cfg.theme.as_deref());
        if now_stamp != last_stamp {
            last_stamp = now_stamp;
            reload = true;
        }
        if reload {
            cfg = load_config();
            theme = load_theme(&cfg);
            (w, h, landscape) = dims(&theme);
            if let Some(l) = lcd.as_mut() {
                setup(l.as_mut(), &cfg, w, h, landscape);
            }
            sens = sensors::Sensors::new(cfg.net_interface.clone(), cfg.disk.clone(), cfg.lhm_url.clone());
            p.accent = hex(&cfg.accent);
            prev = None;
            last_stamp = stamp(cfg.theme.as_deref());
        }
        let interval = Duration::from_millis(cfg.interval_ms.max(200));

        let stats = sens.read();
        let frame = match theme.as_mut() {
            Some(t) => t.render(&stats),
            None => {
                render(&mut p, &stats);
                p.img.clone()
            }
        };
        if let Some(path) = &snapshot {
            let _ = frame.save(path);
        }
        {
            let mut sh = shared.lock().unwrap();
            sh.frame = Some(frame.clone());
            sh.seq += 1;
        }
        // no screen yet, or it was unplugged/reset: keep polling for it, repaint fully once it returns
        let Some(l) = lcd.as_mut() else {
            lcd = open(&cfg, w, h, landscape);
            prev = None;
            thread::sleep(Duration::from_secs(3));
            continue;
        };
        if let Err(e) = push(l.as_mut(), &frame, prev.as_ref(), &mut buf) {
            eprintln!("write failed ({e}), reconnecting...");
            lcd = None;
            continue;
        }
        prev = Some(frame);
        thread::sleep(interval);
    }
}
