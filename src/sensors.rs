use std::time::Instant;
use sysinfo::{Disks, Networks, System};

#[derive(Default, Clone)]
pub struct Gpu {
    pub load: f32,
    pub temp: Option<f32>,
    pub vram_used: u64,
    pub vram_total: u64,
    pub mhz: Option<u64>,
}

#[derive(Default, Clone)]
pub struct Net {
    pub rx_bps: f64,
    pub tx_bps: f64,
    pub rx_total: u64,
    pub tx_total: u64,
}

#[derive(Default, Clone)]
pub struct Stats {
    pub cpu: f32,
    pub cpu_temp: Option<f32>,
    pub cpu_mhz: u64,
    pub load: [f64; 3],
    pub ram_used: u64,
    pub ram_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    pub disk_used: u64,
    pub disk_total: u64,
    /// Primary (auto or configured) interface
    pub net: Net,
    pub wlo: Net,
    pub eth: Net,
    pub gpu: Option<Gpu>,
}

pub struct Sensors {
    sys: System,
    nets: Networks,
    disks: Disks,
    last: Instant,
    iface: Option<String>,
    disk: String,
    lhm_url: Option<String>,
    intel: Option<linux::Intel>,
}

impl Sensors {
    pub fn new(iface: Option<String>, disk: String, lhm_url: Option<String>) -> Self {
        let mut sys = System::new();
        sys.refresh_cpu_usage();
        Self {
            sys,
            nets: Networks::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list(),
            last: Instant::now(),
            iface,
            disk,
            intel: if lhm_url.is_none() { linux::Intel::detect() } else { None },
            lhm_url,
        }
    }

    pub fn read(&mut self) -> Stats {
        let dt = self.last.elapsed().as_secs_f64().max(0.001);
        self.last = Instant::now();
        self.sys.refresh_cpu_all();
        self.sys.refresh_memory();
        self.nets.refresh(true);
        self.disks.refresh(true);

        let la = System::load_average();
        let mut s = Stats {
            cpu: self.sys.global_cpu_usage(),
            cpu_mhz: self.sys.cpus().iter().map(|c| c.frequency()).max().unwrap_or(0),
            load: [la.one, la.five, la.fifteen],
            ram_used: self.sys.used_memory(),
            ram_total: self.sys.total_memory(),
            swap_used: self.sys.used_swap(),
            swap_total: self.sys.total_swap(),
            ..Default::default()
        };

        let net_of = |n: &str| -> Option<Net> {
            let d = self.nets.get(n)?;
            Some(Net {
                rx_bps: d.received() as f64 / dt,
                tx_bps: d.transmitted() as f64 / dt,
                rx_total: d.total_received(),
                tx_total: d.total_transmitted(),
            })
        };
        let busiest = |f: &dyn Fn(&str) -> bool| {
            self.nets
                .iter()
                .filter(|(n, _)| !is_virtual(n) && f(n))
                .max_by_key(|(_, d)| d.total_received() + d.total_transmitted())
                .map(|(n, _)| n.clone())
        };
        let primary = self.iface.clone().or_else(|| busiest(&|_| true));
        s.net = primary.as_deref().and_then(net_of).unwrap_or_default();
        s.wlo = busiest(&|n| n.starts_with("wl")).as_deref().and_then(net_of).unwrap_or_default();
        // no wired interface (laptop on wifi): ETH-only themes still get a live number
        s.eth = busiest(&|n| !n.starts_with("wl")).or_else(|| primary.clone()).as_deref().and_then(net_of).unwrap_or_default();

        if let Some(d) = self.disks.iter().find(|d| d.mount_point().to_string_lossy() == self.disk) {
            s.disk_total = d.total_space();
            s.disk_used = d.total_space() - d.available_space();
        }

        if let Some(url) = &self.lhm_url {
            lhm(url, &mut s);
        } else {
            s.cpu_temp = linux::cpu_temp();
            s.gpu = linux::gpu().or_else(|| self.intel.as_mut().map(|i| i.read()));
        }
        s
    }
}

fn is_virtual(n: &str) -> bool {
    n == "lo" || ["docker", "veth", "br-", "virbr", "vmnet", "tun", "tap"].iter().any(|p| n.starts_with(p))
}

/// LibreHardwareMonitor web server (Options > Remote Web Server), default http://localhost:8085/data.json
fn lhm(url: &str, s: &mut Stats) {
    let Ok(resp) = ureq::get(url).timeout(std::time::Duration::from_millis(800)).call() else { return };
    let Ok(root) = resp.into_json::<serde_json::Value>() else { return };
    let mut gpu = Gpu::default();
    let mut has_gpu = false;
    walk(&root, "", &mut |hw, text, typ, val| {
        let is_gpu = hw.contains("nvidia") || hw.contains("amd") || hw.contains("ati") || hw.contains("intel");
        match (typ, text) {
            ("Temperature", t) if t.contains("CPU Package") || t.contains("Core (Tctl") || t.contains("CPU Core") => {
                s.cpu_temp.get_or_insert(val);
            }
            ("Load", "GPU Core") if is_gpu => { gpu.load = val; has_gpu = true; }
            ("Temperature", "GPU Core") if is_gpu => gpu.temp = Some(val),
            ("Clock", "GPU Core") if is_gpu => gpu.mhz = Some(val as u64),
            ("SmallData", "GPU Memory Used") if is_gpu => gpu.vram_used = (val * 1048576.0) as u64,
            ("SmallData", "GPU Memory Total") if is_gpu => gpu.vram_total = (val * 1048576.0) as u64,
            _ => {}
        }
    });
    if has_gpu {
        s.gpu = Some(gpu);
    }
}

/// Visit every sensor node: (hardware image name, sensor text, sensor type, numeric value)
fn walk(n: &serde_json::Value, hw: &str, f: &mut dyn FnMut(&str, &str, &str, f32)) {
    let img = n["ImageURL"].as_str().unwrap_or("");
    let hw = if img.contains("images_icon/") { img.to_lowercase() } else { hw.to_string() };
    if let (Some(t), Some(ty), Some(v)) = (n["Text"].as_str(), n["Type"].as_str(), n["Value"].as_str()) {
        let num: String = v.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',').collect();
        if let Ok(val) = num.replace(',', ".").parse() {
            f(&hw, t, ty, val);
        }
    }
    if let Some(ch) = n["Children"].as_array() {
        for c in ch {
            walk(c, &hw, f);
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::Gpu;
    use std::collections::HashMap;
    use std::fs;
    use std::time::Instant;

    fn read_num(p: &str) -> Option<f64> {
        fs::read_to_string(p).ok()?.trim().parse().ok()
    }

    pub fn cpu_temp() -> Option<f32> {
        for e in fs::read_dir("/sys/class/hwmon").ok()?.flatten() {
            let p = e.path();
            let name = fs::read_to_string(p.join("name")).unwrap_or_default();
            if matches!(name.trim(), "coretemp" | "k10temp" | "zenpower" | "cpu_thermal") {
                return read_num(&format!("{}/temp1_input", p.display())).map(|v| v as f32 / 1000.0);
            }
        }
        None
    }

    pub fn gpu() -> Option<Gpu> {
        // AMD via sysfs
        for e in fs::read_dir("/sys/class/drm").ok()?.flatten() {
            let d = e.path().join("device");
            let busy = d.join("gpu_busy_percent");
            if busy.exists() {
                let temp = fs::read_dir(d.join("hwmon")).ok().and_then(|mut h| h.next()?.ok()).and_then(|h| {
                    read_num(&format!("{}/temp1_input", h.path().display())).map(|v| v as f32 / 1000.0)
                });
                return Some(Gpu {
                    load: read_num(busy.to_str()?)? as f32,
                    temp,
                    vram_used: read_num(d.join("mem_info_vram_used").to_str()?).unwrap_or(0.0) as u64,
                    vram_total: read_num(d.join("mem_info_vram_total").to_str()?).unwrap_or(0.0) as u64,
                    mhz: None,
                });
            }
        }
        // NVIDIA via nvidia-smi
        let out = std::process::Command::new("nvidia-smi")
            .args(["--query-gpu=utilization.gpu,temperature.gpu,memory.used,memory.total,clocks.gr", "--format=csv,noheader,nounits"])
            .output()
            .ok()?;
        let line = String::from_utf8_lossy(&out.stdout);
        let v: Vec<f64> = line.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        (v.len() >= 4).then(|| Gpu {
            load: v[0] as f32,
            temp: Some(v[1] as f32),
            vram_used: (v[2] * 1048576.0) as u64,
            vram_total: (v[3] * 1048576.0) as u64,
            mhz: v.get(4).map(|m| *m as u64),
        })
    }

    /// Intel iGPU (i915): no busy% in sysfs, so sum per-client render-engine time from
    /// /proc/*/fdinfo and differentiate. Only this user's processes are readable without root,
    /// which on a desktop is the compositor + apps, i.e. nearly everything.
    /// ponytail: i915 only; xe driver reports drm-cycles-rcs instead, add when someone has one.
    pub struct Intel {
        card: String,
        clients: HashMap<u64, u64>,
        last: Instant,
    }

    impl Intel {
        pub fn detect() -> Option<Self> {
            for e in fs::read_dir("/sys/class/drm").ok()?.flatten() {
                let p = e.path();
                let uevent = fs::read_to_string(p.join("device/uevent")).unwrap_or_default();
                if p.file_name()?.to_str()?.starts_with("card") && uevent.contains("DRIVER=i915") {
                    return Some(Self { card: p.to_string_lossy().into(), clients: HashMap::new(), last: Instant::now() });
                }
            }
            None
        }

        pub fn read(&mut self) -> Gpu {
            let mut now: HashMap<u64, u64> = HashMap::new();
            for proc_ in fs::read_dir("/proc").into_iter().flatten().flatten() {
                let Ok(fds) = fs::read_dir(proc_.path().join("fdinfo")) else { continue };
                for fd in fds.flatten() {
                    let Ok(txt) = fs::read_to_string(fd.path()) else { continue };
                    if !txt.contains("drm-driver:\ti915") {
                        continue;
                    }
                    let field = |k: &str| txt.lines().find_map(|l| l.strip_prefix(k)).and_then(|v| v.trim().split(' ').next()?.parse::<u64>().ok());
                    if let (Some(id), Some(ns)) = (field("drm-client-id:"), field("drm-engine-render:")) {
                        now.insert(id, ns);
                    }
                }
            }
            let dt = self.last.elapsed().as_nanos() as f64;
            self.last = Instant::now();
            let busy: u64 = now.iter().map(|(id, ns)| ns.saturating_sub(*self.clients.get(id).unwrap_or(&0))).sum();
            self.clients = now;
            Gpu {
                load: (busy as f64 / dt * 100.0).min(100.0) as f32,
                temp: None,
                vram_used: 0,
                vram_total: 0,
                mhz: read_num(&format!("{}/gt_cur_freq_mhz", self.card)).map(|m| m as u64),
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
mod linux {
    pub fn cpu_temp() -> Option<f32> { None }
    pub fn gpu() -> Option<super::Gpu> { None }
    pub struct Intel;
    impl Intel {
        pub fn detect() -> Option<Self> { None }
        pub fn read(&mut self) -> super::Gpu { Default::default() }
    }
}


#[cfg(all(test, target_os = "linux"))]
mod tests {
    #[test]
    fn intel_fdinfo_reads_nonzero() {
        let Some(mut i) = super::linux::Intel::detect() else { return };
        i.read();
        std::thread::sleep(std::time::Duration::from_secs(1));
        let g = i.read();
        eprintln!("intel load={} mhz={:?}", g.load, g.mhz);
        assert!(g.mhz.is_some());
    }
}
