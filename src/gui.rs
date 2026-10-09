// Settings window (egui), run as its own process (`turing-rs --settings`): a form bound to Config,
// saved to config.toml on every edit (the main process hot-reloads it), plus a live preview read
// from <config>/preview.png, which the main process writes while <config>/preview.want is fresh.
use crate::{config_dir, hex, list_themes, load_config, open_folder, save_config, Config};
use eframe::egui;
use std::path::PathBuf;
use std::time::{Instant, SystemTime};

pub struct App {
    cfg: Config,
    themes: Vec<(String, PathBuf)>,
    tex: Option<egui::TextureHandle>,
    seen: Option<SystemTime>,
    touched: Instant,
    dirty: bool,
}

pub fn run() {
    let icon = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).ok();
    let mut viewport = egui::ViewportBuilder::default().with_inner_size([860.0, 420.0]).with_app_id("turing-rs");
    if let Some(i) = icon {
        viewport = viewport.with_icon(i);
    }
    let opts = eframe::NativeOptions { viewport, ..Default::default() };
    let app = App { cfg: load_config(), themes: list_themes(), tex: None, seen: None, touched: Instant::now() - std::time::Duration::from_secs(10), dirty: false };
    let _ = eframe::run_native("turing-rs", opts, Box::new(|_| Ok(Box::new(app))));
    let _ = std::fs::remove_file(config_dir().join("preview.want"));
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        // keep asking the main process for frames, and pick up new ones
        if self.touched.elapsed().as_secs() >= 2 {
            self.touched = Instant::now();
            let _ = std::fs::write(config_dir().join("preview.want"), b"");
        }
        let png = config_dir().join("preview.png");
        let mtime = png.metadata().and_then(|m| m.modified()).ok();
        if mtime.is_some() && mtime != self.seen {
            if let Ok(f) = image::open(&png) {
                let f = f.to_rgb8();
                let img = egui::ColorImage::from_rgb([f.width() as usize, f.height() as usize], f.as_raw());
                match &mut self.tex {
                    Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                    None => self.tex = Some(ctx.load_texture("preview", img, egui::TextureOptions::LINEAR)),
                }
                self.seen = mtime;
            }
        }

        egui::SidePanel::left("preview").resizable(false).show(ctx, |ui| {
            ui.heading("Preview");
            match &self.tex {
                Some(t) => {
                    let size = t.size_vec2();
                    let scale = (480.0 / size.x).min(360.0 / size.y);
                    ui.image((t.id(), size * scale));
                }
                None => {
                    ui.label("waiting for first frame…");
                }
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Settings");
            let cfg = &mut self.cfg;
            let mut changed = false;
            egui::Grid::new("form").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("Theme");
                let name = cfg.theme.as_ref().and_then(|p| p.file_name()).map_or("Built-in dashboard".to_string(), |n| n.to_string_lossy().into_owned());
                egui::ComboBox::from_id_salt("theme").selected_text(name).show_ui(ui, |ui| {
                    changed |= ui.selectable_value(&mut cfg.theme, None, "Built-in dashboard").changed();
                    for (n, p) in &self.themes {
                        changed |= ui.selectable_value(&mut cfg.theme, Some(p.clone()), n).changed();
                    }
                });
                ui.end_row();

                ui.label("Brightness");
                changed |= ui.add(egui::Slider::new(&mut cfg.brightness, 0..=100).suffix("%")).changed();
                ui.end_row();

                ui.label("Refresh (ms)");
                changed |= ui.add(egui::DragValue::new(&mut cfg.interval_ms).range(200..=10_000).speed(50)).changed();
                ui.end_row();

                ui.label("Accent (built-in)");
                let mut rgb = hex(&cfg.accent).0;
                if egui::color_picker::color_edit_button_srgb(ui, &mut rgb).changed() {
                    cfg.accent = format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]);
                    changed = true;
                }
                ui.end_row();

                let mut opt = |ui: &mut egui::Ui, label: &str, v: &mut Option<String>, hint: &str| {
                    ui.label(label);
                    let mut s = v.clone().unwrap_or_default();
                    if ui.add(egui::TextEdit::singleline(&mut s).hint_text(hint)).changed() {
                        *v = if s.trim().is_empty() { None } else { Some(s) };
                        changed = true;
                    }
                    ui.end_row();
                };
                opt(ui, "Serial port", &mut cfg.port, "auto-detect");
                opt(ui, "Network interface", &mut cfg.net_interface, "auto (busiest)");
                opt(ui, "LibreHardwareMonitor URL", &mut cfg.lhm_url, "http://localhost:8085/data.json");

                ui.label("Disk mount");
                changed |= ui.text_edit_singleline(&mut cfg.disk).changed();
                ui.end_row();
            });

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui.button("Open config folder").clicked() {
                    open_folder(&config_dir());
                }
                if ui.button("Rescan themes").clicked() {
                    self.themes = list_themes();
                }
            });
            ui.add_space(8.0);
            ui.label(egui::RichText::new("Edits save to config.toml and apply to the screen within a second. Theme files are hot-reloaded too.").weak());

            self.dirty |= changed;
        });

        // Save once the user lets go of the slider/picker, not on every drag tick.
        if self.dirty && !ctx.input(|i| i.pointer.any_down()) {
            self.dirty = false;
            save_config(&self.cfg);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}
