// Turing Smart Screen 3.5" (rev A) system monitor.
// Protocol ported from turing-smart-screen-python (GPL-3.0).
mod sensors;
mod theme;

use ab_glyph::{FontRef, PxScale};
use image::{Rgb, RgbImage};
use imageproc::drawing::{draw_filled_rect_mut, draw_text_mut, text_size};
use imageproc::rect::Rect;
use serde::Deserialize;
use std::path::PathBuf;
use std::{io::Write, thread, time::Duration};

pub const MONO: &[u8] = include_bytes!("../assets/LiberationMono-Bold.ttf");
const SANS: &[u8] = include_bytes!("../assets/LiberationSans-Bold.ttf");

#[derive(Deserialize)]
#[serde(default)]
struct Config {
    port: Option<String>,
    brightness: u8,
    interval_ms: u64,
    net_interface: Option<String>,
    disk: String,
    lhm_url: Option<String>,
    accent: String,
    /// Directory containing a turing-smart-screen-python theme.yaml. Unset = built-in dashboard.
    theme: Option<PathBuf>,
    /// Python project's res/fonts dir. Default: <theme>/../../fonts
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

fn load_config() -> Config {
    let path = std::env::args().nth(1).map(Into::into).or_else(|| {
        dirs::config_dir().map(|d| d.join("turing-rs").join("config.toml"))
    });
    let mut cfg: Config = match path.and_then(|p| std::fs::read_to_string(p).ok()) {
        Some(s) => toml::from_str(&s).unwrap_or_else(|e| panic!("bad config: {e}")),
        None => Config::default(),
    };
    for p in [&mut cfg.theme, &mut cfg.fonts_dir].into_iter().flatten() {
        if let (Ok(rest), Some(home)) = (p.strip_prefix("~"), dirs::home_dir()) {
            *p = home.join(rest);
        }
    }
    cfg
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

fn open(cfg: &Config, w: u16, h: u16, landscape: bool) -> Box<dyn serialport::SerialPort> {
    let port = cfg.port.clone().or_else(find_port).expect("screen not found; set `port` in config");
    let mut lcd = serialport::new(&port, 115200)
        .timeout(Duration::from_secs(2))
        .flow_control(serialport::FlowControl::Hardware)
        .open()
        .expect("open serial");
    eprintln!("using {port}");
    // orientation (portrait 0 / landscape 2) + 100, then width/height big-endian, padded to 16 bytes
    let mut o = [0u8; 16];
    o[5] = 121;
    o[6] = 100 + if landscape { 2 } else { 0 };
    o[7..9].copy_from_slice(&w.to_be_bytes());
    o[9..11].copy_from_slice(&h.to_be_bytes());
    lcd.write_all(&o).unwrap();
    // brightness: 0 = brightest, 255 = darkest
    lcd.write_all(&cmd(255 - (cfg.brightness.min(100) as u16 * 255 / 100), 0, 0, 0, 110)).unwrap();
    lcd
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
    let cfg = load_config();
    let mut theme = cfg.theme.as_ref().map(|d| {
        theme::Theme::load(d, cfg.fonts_dir.clone()).unwrap_or_else(|e| panic!("theme: {e}"))
    });
    let (w, h, landscape) = match &theme {
        Some(t) => (t.width as u16, t.height as u16, t.landscape),
        None => (W, H, true),
    };
    let mut lcd = open(&cfg, w, h, landscape);
    let mut sens = sensors::Sensors::new(cfg.net_interface.clone(), cfg.disk.clone(), cfg.lhm_url.clone());
    let mut p = Painter {
        img: RgbImage::new(W as u32, H as u32),
        mono: FontRef::try_from_slice(MONO).unwrap(),
        sans: FontRef::try_from_slice(SANS).unwrap(),
        accent: hex(&cfg.accent),
    };
    let mut buf = Vec::with_capacity(w as usize * h as usize * 2);
    let interval = Duration::from_millis(cfg.interval_ms.max(200));
    let snapshot = std::env::var("TURING_PNG").ok(); // debug: also write each frame here
    let mut prev: Option<RgbImage> = None;

    loop {
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
        if let Err(e) = push(lcd.as_mut(), &frame, prev.as_ref(), &mut buf) {
            // screen unplugged / reset: retry until it comes back, then repaint everything
            eprintln!("write failed ({e}), reconnecting...");
            prev = None;
            thread::sleep(Duration::from_secs(3));
            if let Ok(l) = std::panic::catch_unwind(|| open(&cfg, w, h, landscape)) {
                lcd = l;
            }
            continue;
        }
        prev = Some(frame);
        thread::sleep(interval);
    }
}
