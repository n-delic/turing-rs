// Renders turing-smart-screen-python theme.yaml files onto a full frame.
// Semantics mirror library/stats.py + lcd_comm.py (text anchors, bar fill, PIL arc angles).
use crate::sensors::Stats;
use ab_glyph::{Font, FontVec, PxScale, ScaleFont};
use image::{Rgb, RgbImage};
use imageproc::drawing::draw_text_mut;
use serde_yaml::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct Theme {
    pub width: u32,
    pub height: u32,
    pub landscape: bool,
    yaml: Value,
    dir: PathBuf,
    fonts_dir: PathBuf,
    fonts: HashMap<String, FontVec>,
    images: HashMap<String, RgbImage>,
    background: RgbImage,
}

type Color = Rgb<u8>;

fn color(v: &Value, default: Color) -> Color {
    let Some(s) = v.as_str() else { return default };
    let p: Vec<u8> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
    if p.len() == 3 { Rgb([p[0], p[1], p[2]]) } else { default }
}
fn num(v: &Value, d: f64) -> f64 {
    v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)).unwrap_or(d)
}
fn flag(v: &Value, d: bool) -> bool {
    v.as_bool().or_else(|| v.as_str().map(|s| s.eq_ignore_ascii_case("true"))).unwrap_or(d)
}
fn shown(v: &Value) -> bool {
    !v.is_null() && flag(&v["SHOW"], false)
}

impl Theme {
    pub fn load(dir: &Path, fonts_dir: Option<PathBuf>) -> Result<Self, String> {
        let text = std::fs::read_to_string(dir.join("theme.yaml")).map_err(|e| format!("theme.yaml: {e}"))?;
        let yaml: Value = serde_yaml::from_str(&text).map_err(|e| format!("theme.yaml: {e}"))?;
        let landscape = yaml["display"]["DISPLAY_ORIENTATION"].as_str().map_or(false, |o| o.contains("landscape"));
        // Default to 3.5" size; a static BACKGROUND image, if present, overrides.
        let (mut width, mut height) = if landscape { (480, 320) } else { (320, 480) };
        let bg = &yaml["static_images"]["BACKGROUND"];
        if !bg.is_null() {
            width = num(&bg["WIDTH"], width as f64) as u32;
            height = num(&bg["HEIGHT"], height as f64) as u32;
        }
        let fonts_dir = fonts_dir
            .or_else(|| {
                let cfg_fonts = dirs::config_dir().map(|c| c.join("turing-rs/fonts"));
                [Some(dir.join("../../fonts")), Some(dir.join("fonts")), cfg_fonts].into_iter().flatten().find(|p| p.is_dir())
            })
            .unwrap_or_else(|| dir.to_path_buf());
        let mut t = Self {
            width,
            height,
            landscape,
            yaml,
            dir: dir.to_path_buf(),
            fonts_dir,
            fonts: HashMap::new(),
            images: HashMap::new(),
            background: RgbImage::new(width, height),
        };
        t.background = t.static_layer();
        Ok(t)
    }

    /// Background + static images + static text — everything that never changes.
    fn static_layer(&mut self) -> RgbImage {
        let mut img = RgbImage::new(self.width, self.height);
        if let Some(m) = self.yaml["static_images"].as_mapping().cloned() {
            for (_, v) in m {
                let Some(path) = v["PATH"].as_str() else { continue };
                let (x, y) = (num(&v["X"], 0.0) as i64, num(&v["Y"], 0.0) as i64);
                if let Some(src) = self.image(path).cloned() {
                    let (w, h) = (num(&v["WIDTH"], src.width() as f64) as u32, num(&v["HEIGHT"], src.height() as f64) as u32);
                    let src = if (w, h) != src.dimensions() {
                        image::imageops::resize(&src, w, h, image::imageops::FilterType::Triangle)
                    } else {
                        src
                    };
                    image::imageops::overlay(&mut img, &src, x, y);
                }
            }
        }
        if let Some(m) = self.yaml["static_text"].as_mapping().cloned() {
            for (_, v) in m {
                if let Some(s) = v["TEXT"].as_str() {
                    // static text has no SHOW key; always drawn
                    self.text(&mut img, &v, s, true);
                }
            }
        }
        img
    }

    fn image(&mut self, name: &str) -> Option<&RgbImage> {
        if !self.images.contains_key(name) {
            let img = image::open(self.dir.join(name)).ok()?.to_rgb8();
            self.images.insert(name.to_string(), img);
        }
        self.images.get(name)
    }

    fn font(&mut self, name: &str) -> &FontVec {
        if !self.fonts.contains_key(name) {
            let f = std::fs::read(self.fonts_dir.join(name))
                .ok()
                .and_then(|b| FontVec::try_from_vec(b).ok())
                .unwrap_or_else(|| {
                    eprintln!("font {name} not found in {}, using built-in", self.fonts_dir.display());
                    FontVec::try_from_vec(crate::MONO.to_vec()).unwrap()
                });
            self.fonts.insert(name.to_string(), f);
        }
        &self.fonts[name]
    }

    /// Theme element has no BACKGROUND_IMAGE → it paints a solid BACKGROUND_COLOR box first.
    fn solid_bg(&self, v: &Value, img: &mut RgbImage, x: i32, y: i32, w: u32, h: u32, default: Color) {
        if v["BACKGROUND_IMAGE"].is_null() && w > 0 && h > 0 {
            let c = color(&v["BACKGROUND_COLOR"], default);
            imageproc::drawing::draw_filled_rect_mut(img, imageproc::rect::Rect::at(x, y).of_size(w, h), c);
        }
    }

    // ---- primitives ----

    fn text(&mut self, img: &mut RgbImage, v: &Value, s: &str, force: bool) {
        if !force && !shown(v) {
            return;
        }
        let size = num(&v["FONT_SIZE"], 10.0) as f32;
        let fname = v["FONT"].as_str().unwrap_or("roboto-mono/RobotoMono-Regular.ttf").to_string();
        let fc = color(&v["FONT_COLOR"], Rgb([0, 0, 0]));
        let anchor = v["ANCHOR"].as_str().unwrap_or("lt").as_bytes().to_vec();
        let (mut x, mut y) = (num(&v["X"], 0.0) as i32, num(&v["Y"], 0.0) as i32);
        let (bw, mut bh) = (num(&v["WIDTH"], 0.0) as u32, num(&v["HEIGHT"], 0.0) as u32);
        if bw > 0 && bh == 0 {
            bh = size as u32;
        }
        // With a fixed box, anchor refers to the box, then the text is anchored at that point (PIL semantics).
        if bw > 0 && bh > 0 {
            match anchor.first() {
                Some(b'm') => x += bw as i32 / 2,
                Some(b'r') => x += bw as i32,
                _ => {}
            }
            match anchor.get(1) {
                Some(b'm') => y += bh as i32 / 2,
                Some(b'b') | Some(b'd') => y += bh as i32,
                _ => {}
            }
            let bg = color(&v["BACKGROUND_COLOR"], Rgb([255, 255, 255]));
            self.solid_bg(v, img, num(&v["X"], 0.0) as i32, num(&v["Y"], 0.0) as i32, bw, bh, bg);
        }
        let font = self.font(&fname);
        let scale = font.pt_to_px_scale(size).unwrap_or(PxScale::from(size));
        let sf = font.as_scaled(scale);
        let w = text_width(&sf, s);
        let (asc, desc) = (sf.ascent(), sf.descent());
        let h = asc - desc;
        let dx = match anchor.first() {
            Some(b'm') => -w / 2.0,
            Some(b'r') => -w,
            _ => 0.0,
        };
        // draw_text_mut puts the ascender line at y; PIL anchors: a/t=ascender, m=middle, s=baseline, b/d=descender
        let dy = match anchor.get(1) {
            Some(b'm') => -h / 2.0,
            Some(b's') => -asc,
            Some(b'b') | Some(b'd') => -h,
            _ => 0.0,
        };
        let font = self.font(&fname);
        draw_text_mut(img, fc, x + dx.round() as i32, y + dy.round() as i32, scale, font, s);
    }

    fn bar(&mut self, img: &mut RgbImage, v: &Value, value: f64) {
        if !shown(v) {
            return;
        }
        let (x, y) = (num(&v["X"], 0.0) as i32, num(&v["Y"], 0.0) as i32);
        let (w, h) = (num(&v["WIDTH"], 0.0) as i32, num(&v["HEIGHT"], 0.0) as i32);
        if w <= 0 || h <= 0 {
            return;
        }
        let (min, max) = (num(&v["MIN_VALUE"], 0.0), num(&v["MAX_VALUE"], 100.0));
        let value = value.clamp(min, max);
        let bc = color(&v["BAR_COLOR"], Rgb([0, 0, 0]));
        let rev = flag(&v["REVERSE_DIRECTION"], false);
        self.solid_bg(v, img, x, y, w as u32, h as u32, Rgb([255, 255, 255]));
        let frac = (value - min) / (max - min).max(1e-9);
        let (mut x1, mut y1, mut x2, mut y2) = (0, 0, w - 1, h - 1);
        if w > h {
            let fill = ((frac * w as f64) - 1.0).max(0.0) as i32;
            if rev { x1 = w - 1 - fill } else { x2 = fill }
        } else {
            let fill = ((frac * h as f64) - 1.0).max(0.0) as i32;
            if rev { y2 = fill } else { y1 = h - 1 - fill }
        }
        fill_rect_inclusive(img, x + x1, y + y1, x + x2, y + y2, bc);
        if flag(&v["BAR_OUTLINE"], false) {
            imageproc::drawing::draw_hollow_rect_mut(img, imageproc::rect::Rect::at(x, y).of_size(w as u32, h as u32), bc);
        }
    }

    fn radial(&mut self, img: &mut RgbImage, v: &Value, value: f64, label: Option<String>) {
        if !shown(v) {
            return;
        }
        let (xc, yc) = (num(&v["X"], 0.0) as i32, num(&v["Y"], 0.0) as i32);
        let r = num(&v["RADIUS"], 1.0) as i32;
        let bw = num(&v["WIDTH"], 1.0) as i32;
        let (min, max) = (num(&v["MIN_VALUE"], 0.0), num(&v["MAX_VALUE"], 100.0));
        let pct = (value.clamp(min, max) - min) / (max - min).max(1e-9);
        let (mut a0, mut a1) = (num(&v["ANGLE_START"], 0.0), num(&v["ANGLE_END"], 360.0));
        let steps = num(&v["ANGLE_STEPS"], 1.0).max(1.0);
        let sep = num(&v["ANGLE_SEP"], 0.0);
        let cw = flag(&v["CLOCKWISE"], false);
        if a0 % 361.0 == a1 % 361.0 {
            if cw { a0 += 0.1 } else { a1 += 0.1 }
        }
        a0 %= 361.0;
        a1 %= 361.0;
        self.solid_bg(v, img, xc - r, yc - r, 2 * r as u32, 2 * r as u32, Rgb([0, 0, 0]));

        // Build the list of (start, end) arcs in PIL convention: degrees clockwise from 3 o'clock.
        let mut arcs: Vec<(f64, f64)> = Vec::new();
        let span = if cw {
            if a1 < a0 { 360.0 - a0 + a1 } else { a1 - a0 }
        } else if a1 < a0 {
            a0 - a1
        } else {
            360.0 - a1 + a0
        };
        let bar_bg = color(&v["BAR_BACKGROUND_COLOR"], Rgb([0, 0, 0]));
        if flag(&v["DRAW_BAR_BACKGROUND"], false) {
            let bg_arc = if cw { (a0, a0 + span) } else { (a0 - span, a0) };
            draw_arc(img, xc, yc, r, bw, bg_arc.0, bg_arc.1, bar_bg);
        }
        let step = span / steps;
        if sep == 0.0 {
            arcs.push(if cw { (a0, a0 + pct * span) } else { (a0 - pct * span, a0) });
        } else if cw {
            let end = a0 + pct * span;
            let n = ((end - a0) / step) as i32;
            for i in 0..n {
                arcs.push((a0 + i as f64 * step, a0 + (i + 1) as f64 * step - sep));
            }
            arcs.push((a0 + n as f64 * step, end));
        } else {
            let start = a0 - pct * span;
            let n = ((a0 - start) / step) as i32;
            for i in 0..n {
                arcs.push((a0 - (i + 1) as f64 * step + sep, a0 - i as f64 * step));
            }
            arcs.push((start, a0 - n as f64 * step));
        }
        let bc = color(&v["BAR_COLOR"], Rgb([0, 0, 0]));
        for (s, e) in arcs {
            draw_arc(img, xc, yc, r, bw, s, e, bc);
        }

        // Text centred on its ink box, like PIL's getbbox-based placement.
        let text = if flag(&v["SHOW_TEXT"], false) {
            label.unwrap_or_else(|| format!("{}%", (pct * 100.0 + 0.5) as i32))
        } else {
            String::new()
        };
        if !text.is_empty() {
            let size = num(&v["FONT_SIZE"], 10.0) as f32;
            let fname = v["FONT"].as_str().unwrap_or("roboto-mono/RobotoMono-Regular.ttf").to_string();
            let fc = color(&v["FONT_COLOR"], Rgb([0, 0, 0]));
            let (ox, oy) = v["TEXT_OFFSET"].as_str().map_or((0.0, 0.0), |s| {
                let p: Vec<f64> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                (p.first().copied().unwrap_or(0.0), p.get(1).copied().unwrap_or(0.0))
            });
            let font = self.font(&fname);
            let scale = font.pt_to_px_scale(size).unwrap_or(PxScale::from(size));
            let sf = font.as_scaled(scale);
            let w = text_width(&sf, &text);
            let (top, bottom) = ink_bounds(&sf, &text);
            let x = xc as f64 - w as f64 / 2.0 + ox;
            let y = yc as f64 - top as f64 - (bottom - top) as f64 / 2.0 + oy;
            draw_text_mut(img, fc, x.round() as i32, y.round() as i32, scale, font, &text);
        }
    }

    // ---- stat helpers mirroring stats.py ----

    fn value(&mut self, img: &mut RgbImage, v: &Value, val: impl std::fmt::Display, min: usize, unit: &str) {
        if !shown(v) {
            return;
        }
        let min = num(&v["MIN_SIZE"], min as f64) as usize;
        let mut s = format!("{:>w$}", val.to_string(), w = min);
        if flag(&v["SHOW_UNIT"], true) {
            s.push_str(unit);
        }
        self.text(img, v, &s, false);
    }

    fn radial_value(&mut self, img: &mut RgbImage, v: &Value, val: f64, shown_val: impl std::fmt::Display, min: usize, unit: &str) {
        if !shown(v) {
            return;
        }
        let mut s = format!("{:>w$}", shown_val.to_string(), w = min);
        if flag(&v["SHOW_UNIT"], true) {
            s.push_str(unit);
        }
        self.radial(img, v, val, Some(s));
    }

    /// percent-type stat node: TEXT / GRAPH / RADIAL
    fn pct(&mut self, img: &mut RgbImage, node: &Value, val: f64) {
        self.value(img, &node["TEXT"], val as i64, 3, "%");
        self.bar(img, &node["GRAPH"], val);
        self.radial_value(img, &node["RADIAL"], val, val as i64, 3, "%");
    }
    fn temp(&mut self, img: &mut RgbImage, node: &Value, val: Option<f32>) {
        let Some(t) = val else { return };
        self.value(img, &node["TEXT"], t as i64, 3, "°C");
        self.bar(img, &node["GRAPH"], t as f64);
        self.radial_value(img, &node["RADIAL"], t as f64, t as i64, 3, "°C");
    }
    fn ghz(&mut self, img: &mut RgbImage, node: &Value, mhz: u64) {
        let g = mhz as f64 / 1000.0;
        self.value(img, &node["TEXT"], format!("{g:.2}"), 4, " GHz");
        self.bar(img, &node["GRAPH"], g);
        self.radial_value(img, &node["RADIAL"], g, format!("{g:.2}"), 4, " GHz");
    }

    pub fn render(&mut self, s: &Stats) -> RgbImage {
        let mut img = self.background.clone();
        let st = self.yaml["STATS"].clone();

        // CPU
        let cpu = &st["CPU"];
        self.pct(&mut img, &cpu["PERCENTAGE"], s.cpu as f64);
        self.ghz(&mut img, &cpu["FREQUENCY"], s.cpu_mhz);
        self.temp(&mut img, &cpu["TEMPERATURE"], s.cpu_temp);
        for (k, i) in [("ONE", 0), ("FIVE", 1), ("FIFTEEN", 2)] {
            self.value(&mut img, &cpu["LOAD"][k]["TEXT"], s.load[i] as i64, 3, "%");
        }

        // GPU
        let gpu = &st["GPU"];
        if let Some(g) = &s.gpu {
            self.pct(&mut img, &gpu["PERCENTAGE"], g.load as f64);
            let mem_pct = if g.vram_total > 0 { g.vram_used as f64 / g.vram_total as f64 * 100.0 } else { 0.0 };
            let (used_mb, total_mb) = (g.vram_used / 1048576, g.vram_total / 1048576);
            self.pct(&mut img, &gpu["MEMORY_PERCENT"], mem_pct);
            self.bar(&mut img, &gpu["MEMORY"]["GRAPH"], mem_pct);
            self.radial_value(&mut img, &gpu["MEMORY"]["RADIAL"], mem_pct, mem_pct as i64, 3, "%");
            self.value(&mut img, &gpu["MEMORY"]["TEXT"], used_mb, 5, " M");
            self.value(&mut img, &gpu["MEMORY_USED"]["TEXT"], used_mb, 5, " M");
            self.value(&mut img, &gpu["MEMORY_TOTAL"]["TEXT"], total_mb, 5, " M");
            self.temp(&mut img, &gpu["TEMPERATURE"], g.temp);
            if let Some(m) = g.mhz {
                self.ghz(&mut img, &gpu["FREQUENCY"], m);
            }
        }

        // MEMORY
        let mem = &st["MEMORY"];
        let swap_pct = if s.swap_total > 0 { s.swap_used as f64 / s.swap_total as f64 * 100.0 } else { 0.0 };
        self.bar(&mut img, &mem["SWAP"]["GRAPH"], swap_pct);
        self.radial_value(&mut img, &mem["SWAP"]["RADIAL"], swap_pct, swap_pct as i64, 3, "%");
        let ram_pct = s.ram_used as f64 / s.ram_total.max(1) as f64 * 100.0;
        let virt = &mem["VIRTUAL"];
        self.bar(&mut img, &virt["GRAPH"], ram_pct);
        self.radial_value(&mut img, &virt["RADIAL"], ram_pct, ram_pct as i64, 3, "%");
        self.value(&mut img, &virt["PERCENT_TEXT"], ram_pct as i64, 3, "%");
        self.value(&mut img, &virt["USED"], s.ram_used / 1048576, 5, " M");
        self.value(&mut img, &virt["FREE"], (s.ram_total - s.ram_used) / 1048576, 5, " M");
        self.value(&mut img, &virt["TOTAL"], s.ram_total / 1048576, 5, " M");

        // DISK (python uses decimal GB)
        let disk = &st["DISK"];
        let disk_pct = s.disk_used as f64 / s.disk_total.max(1) as f64 * 100.0;
        self.bar(&mut img, &disk["USED"]["GRAPH"], disk_pct);
        self.radial_value(&mut img, &disk["USED"]["RADIAL"], disk_pct, disk_pct as i64, 3, "%");
        self.value(&mut img, &disk["USED"]["PERCENT_TEXT"], disk_pct as i64, 3, "%");
        self.value(&mut img, &disk["USED"]["TEXT"], s.disk_used / 1_000_000_000, 5, " G");
        self.value(&mut img, &disk["TOTAL"]["TEXT"], s.disk_total / 1_000_000_000, 5, " G");
        self.value(&mut img, &disk["FREE"]["TEXT"], (s.disk_total - s.disk_used) / 1_000_000_000, 5, " G");

        // NET
        for (k, n) in [("WLO", &s.wlo), ("ETH", &s.eth)] {
            let node = &st["NET"][k];
            self.value(&mut img, &node["UPLOAD"]["TEXT"], bytes_human(n.tx_bps, true), 10, "");
            self.value(&mut img, &node["DOWNLOAD"]["TEXT"], bytes_human(n.rx_bps, true), 10, "");
            self.value(&mut img, &node["UPLOADED"]["TEXT"], bytes_human(n.tx_total as f64, false), 6, "");
            self.value(&mut img, &node["DOWNLOADED"]["TEXT"], bytes_human(n.rx_total as f64, false), 6, "");
        }

        // DATE
        let now = chrono::Local::now();
        let date = &st["DATE"];
        let day_fmt = match date["DAY"]["FORMAT"].as_str().unwrap_or("medium") {
            "short" => "%-m/%-d/%y".to_string(),
            "medium" => "%b %-d, %Y".to_string(),
            "long" => "%B %-d, %Y".to_string(),
            "full" => "%A, %B %-d, %Y".to_string(),
            custom => babel_to_strftime(custom),
        };
        let hour_fmt = match date["HOUR"]["FORMAT"].as_str().unwrap_or("short") {
            "short" => "%-I:%M %p".to_string(),
            "medium" | "long" | "full" => "%-I:%M:%S %p".to_string(),
            custom => babel_to_strftime(custom),
        };
        self.value(&mut img, &date["DAY"]["TEXT"], now.format(&day_fmt), 0, "");
        self.value(&mut img, &date["HOUR"]["TEXT"], now.format(&hour_fmt), 0, "");

        // UPTIME
        let up = sysinfo::System::uptime();
        let upt = &st["UPTIME"];
        self.value(&mut img, &upt["SECONDS"]["TEXT"], up, 0, "");
        self.value(&mut img, &upt["FORMATTED"]["TEXT"], format!("{:02}:{:02}:{:02}", up / 3600, up % 3600 / 60, up % 60), 0, "");

        img
    }
}

fn text_width<F: Font>(sf: &ab_glyph::PxScaleFont<F>, s: &str) -> f32 {
    let mut w = 0.0;
    let mut prev: Option<ab_glyph::GlyphId> = None;
    for c in s.chars() {
        let id = sf.glyph_id(c);
        if let Some(p) = prev {
            w += sf.kern(p, id);
        }
        w += sf.h_advance(id);
        prev = Some(id);
    }
    w
}

/// Ink top/bottom relative to the ascender line (what PIL's getbbox returns for anchor 'la').
fn ink_bounds<F: Font>(sf: &ab_glyph::PxScaleFont<F>, s: &str) -> (f32, f32) {
    let asc = sf.ascent();
    let (mut top, mut bottom) = (f32::MAX, f32::MIN);
    for c in s.chars() {
        let g = sf.scaled_glyph(c);
        if let Some(o) = sf.outline_glyph(g) {
            let b = o.px_bounds();
            top = top.min(asc + b.min.y);
            bottom = bottom.max(asc + b.max.y);
        }
    }
    if top > bottom { (0.0, sf.height()) } else { (top, bottom) }
}

fn fill_rect_inclusive(img: &mut RgbImage, x1: i32, y1: i32, x2: i32, y2: i32, c: Color) {
    for y in y1.max(0)..=y2.min(img.height() as i32 - 1) {
        for x in x1.max(0)..=x2.min(img.width() as i32 - 1) {
            img.put_pixel(x as u32, y as u32, c);
        }
    }
}

/// Thick arc in PIL convention: angles in degrees, 0 = 3 o'clock, increasing clockwise (y down).
fn draw_arc(img: &mut RgbImage, xc: i32, yc: i32, r: i32, width: i32, start: f64, end: f64, c: Color) {
    if end <= start {
        return;
    }
    let span = (end - start).min(360.0);
    let (ro, ri) = (r as f64 - 0.5, (r - width) as f64 - 0.5);
    for py in (yc - r).max(0)..(yc + r).min(img.height() as i32) {
        for px in (xc - r).max(0)..(xc + r).min(img.width() as i32) {
            let (dx, dy) = (px as f64 - xc as f64 + 0.5, py as f64 - yc as f64 + 0.5);
            let d = (dx * dx + dy * dy).sqrt();
            if d > ro || d <= ri {
                continue;
            }
            let a = dy.atan2(dx).to_degrees().rem_euclid(360.0);
            if (a - start).rem_euclid(360.0) <= span {
                img.put_pixel(px as u32, py as u32, c);
            }
        }
    }
}

/// psutil bytes2human: "12.3 M/s" for rates, "12.3M" for totals.
fn bytes_human(n: f64, rate: bool) -> String {
    let syms = ["K", "M", "G", "T", "P"];
    for (i, s) in syms.iter().enumerate().rev() {
        let unit = 1024f64.powi(i as i32 + 1);
        if n >= unit {
            return if rate { format!("{:.1} {s}/s", n / unit) } else { format!("{:.1}{s}", n / unit) };
        }
    }
    if rate { format!("{n:.1} B/s") } else { format!("{n:.0}B") }
}

fn babel_to_strftime(p: &str) -> String {
    [("yyyy", "%Y"), ("yy", "%y"), ("MMMM", "%B"), ("MMM", "%b"), ("MM", "%m"), ("dd", "%d"), ("d", "%-d"),
     ("EEEE", "%A"), ("EEE", "%a"), ("HH", "%H"), ("hh", "%I"), ("h", "%-I"), ("mm", "%M"), ("ss", "%S"), ("a", "%p"), ("zzz", "%Z")]
        .iter()
        .fold(p.to_string(), |acc, (k, v)| acc.replace(k, v))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn human() {
        assert_eq!(bytes_human(1536.0, true), "1.5 K/s");
        assert_eq!(bytes_human(3.0 * 1048576.0, false), "3.0M");
        assert_eq!(bytes_human(12.0, true), "12.0 B/s");
    }
    #[test]
    fn arc_covers_expected_quadrant() {
        let mut img = RgbImage::new(100, 100);
        draw_arc(&mut img, 50, 50, 40, 10, 0.0, 90.0, Rgb([255, 0, 0]));
        assert_eq!(img.get_pixel(85, 50).0, [255, 0, 0]); // 0° = 3 o'clock, just inside outer radius
        assert_eq!(img.get_pixel(50, 85).0, [255, 0, 0]); // 90° = 6 o'clock (y down)
        assert_eq!(img.get_pixel(15, 50).0, [0, 0, 0]); // 180° not drawn
    }
}
