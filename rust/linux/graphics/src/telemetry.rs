//! On-demand hardware samples. The host owns polling and keeps the reads off stream threads.
//! On Linux a sample carries the CPU usage from `/proc/stat` and, for each GPU the `amdgpu`
//! driver runs, what it publishes in sysfs (`gpu_busy_percent`, `mem_info_vram_*` and its
//! hwmon files), under the Windows host's metric names. Other GPUs are not listed.
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub use butterpollo_config::config::HARDWARE_METRICS as METRICS;

#[derive(Clone, Debug, Default, Serialize)]
pub struct Sample {
    pub cpu: BTreeMap<String, f64>,
    pub gpus: Vec<GpuSample>,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct GpuSample {
    pub id: i32,
    pub name: String,
    pub metrics: BTreeMap<String, f64>,
}

pub struct Collector {
    selected: Vec<String>,
    /// The CPU's idle and total times at the last sample.
    cpu: Option<[u64; 2]>,
    /// The AMD GPUs: id (the render node's minor number), name and sysfs device folder.
    gpus: Vec<(i32, String, PathBuf)>,
}
impl Collector {
    /// Construct, sample and drop on the host's single telemetry worker.
    pub fn new(selected: &[String]) -> Self {
        Self {
            selected: selected.to_vec(),
            cpu: wanted(selected, "cpu_usage_percent")
                .then(cpu_times)
                .flatten(),
            gpus: if selected.is_empty() || selected.iter().any(|name| name.starts_with("gpu_")) {
                amd_gpus()
            } else {
                Vec::new()
            },
        }
    }
    /// The CPU usage since the last sample and each AMD GPU's readings, those selected and
    /// readable.
    pub fn sample(&mut self) -> Sample {
        let mut sample = Sample::default();
        if wanted(&self.selected, "cpu_usage_percent") {
            let next = cpu_times();
            if let Some(value) = self
                .cpu
                .zip(next)
                .and_then(|(old, new)| cpu_usage(old, new))
            {
                sample.cpu.insert("cpu_usage_percent".into(), value);
            }
            self.cpu = next;
        }
        sample.gpus = self
            .gpus
            .iter()
            .map(|(id, name, device)| GpuSample {
                id: *id,
                name: name.clone(),
                metrics: amd_metrics(device, &self.selected),
            })
            .collect();
        sample
    }
}

/// The render nodes whose driver is `amdgpu`, with their sysfs device folders.
fn amd_gpus() -> Vec<(i32, String, PathBuf)> {
    crate::vaapi::node::nodes()
        .iter()
        .filter_map(|node| {
            let device = PathBuf::from(format!("/sys/class/drm/renderD{}/device", node.minor));
            let driver = std::fs::read_link(device.join("driver")).ok()?;
            (driver.file_name()? == "amdgpu")
                .then(|| (node.minor as i32, node.name.clone(), device))
        })
        .collect()
}

/// What `amdgpu` publishes for the GPU at `device`, in the Windows host's names and units.
/// A file that is missing or unreadable leaves its metric out, as ADLX's unsupported ones are.
fn amd_metrics(device: &Path, selected: &[String]) -> BTreeMap<String, f64> {
    let read =
        |path: &Path| -> Option<f64> { std::fs::read_to_string(path).ok()?.trim().parse().ok() };
    let mut metrics = BTreeMap::new();
    let mut add = |name: &str, value: Option<f64>| {
        if wanted(selected, name)
            && let Some(value) = value.filter(|value| value.is_finite())
        {
            metrics.insert(name.to_owned(), value);
        }
    };
    add("gpu_usage_percent", read(&device.join("gpu_busy_percent")));
    let (used, total) = (
        read(&device.join("mem_info_vram_used")),
        read(&device.join("mem_info_vram_total")),
    );
    add("gpu_vram_used_mb", used.map(|bytes| bytes / 1_048_576.));
    add(
        "gpu_vram_usage_percent",
        used.zip(total.filter(|total| *total > 0.))
            .map(|(used, total)| 100. * used / total),
    );
    let Some(hwmon) = std::fs::read_dir(device.join("hwmon"))
        .ok()
        .and_then(|mut entries| entries.next())
        .and_then(|entry| entry.ok())
        .map(|entry| entry.path())
    else {
        return metrics;
    };
    // A labelled reading: the `<kind><n>_input` whose `<kind><n>_label` says `label`. hwmon
    // counts voltages from `in0`, the rest from 1.
    let labelled = |kind: &str, label: &str| -> Option<f64> {
        (0..=8).find_map(|n| {
            let named = std::fs::read_to_string(hwmon.join(format!("{kind}{n}_label"))).ok()?;
            (named.trim() == label).then(|| read(&hwmon.join(format!("{kind}{n}_input"))))?
        })
    };
    for (name, label) in [
        ("gpu_temperature_c", "edge"),
        ("gpu_hotspot_temperature_c", "junction"),
        ("gpu_memory_temperature_c", "mem"),
    ] {
        add(
            name,
            labelled("temp", label).map(|millidegrees| millidegrees / 1000.),
        );
    }
    add("gpu_clock_mhz", labelled("freq", "sclk").map(|hz| hz / 1e6));
    add(
        "gpu_memory_clock_mhz",
        labelled("freq", "mclk").map(|hz| hz / 1e6),
    );
    add("gpu_voltage_mv", labelled("in", "vddgfx"));
    // The package power (`PPT`), averaged by the driver, in microwatts.
    let power = read(&hwmon.join("power1_average")).or_else(|| read(&hwmon.join("power1_input")));
    add("gpu_power_w", power.map(|microwatts| microwatts / 1e6));
    add("gpu_fan_rpm", read(&hwmon.join("fan1_input")));
    add(
        "gpu_fan_percent",
        read(&hwmon.join("pwm1"))
            .zip(read(&hwmon.join("pwm1_max")).filter(|max| *max > 0.))
            .map(|(duty, max)| 100. * duty / max),
    );
    metrics
}
fn wanted(selected: &[String], metric: &str) -> bool {
    selected.is_empty() || selected.iter().any(|name| name == metric)
}
/// The idle and total CPU time of all processors, in the kernel's ticks.
fn cpu_times() -> Option<[u64; 2]> {
    parse_cpu_times(&std::fs::read_to_string("/proc/stat").ok()?)
}
/// The first line of `/proc/stat`: user, nice, system, idle, iowait, irq, softirq and steal
/// times. Time waiting for I/O counts as idle; guest time is already part of user time.
fn parse_cpu_times(stat: &str) -> Option<[u64; 2]> {
    let mut fields = stat.lines().next()?.split_ascii_whitespace();
    if fields.next()? != "cpu" {
        return None;
    }
    let times: Vec<u64> = fields
        .take(8)
        .map(|field| field.parse().ok())
        .collect::<Option<_>>()?;
    let idle = times
        .get(3)?
        .checked_add(times.get(4).copied().unwrap_or(0))?;
    let total = times
        .iter()
        .try_fold(0u64, |total, time| total.checked_add(*time))?;
    Some([idle, total])
}
fn cpu_usage(old: [u64; 2], new: [u64; 2]) -> Option<f64> {
    let idle = new[0].checked_sub(old[0])?;
    let total = new[1].checked_sub(old[1])?;
    (total > 0 && idle <= total).then(|| 100. * (total - idle) as f64 / total as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_usage_subtracts_idle_from_the_total_and_skips_unusable_deltas() {
        assert_eq!(cpu_usage([10, 30], [20, 60]), Some(100. * 20. / 30.));
        assert_eq!(cpu_usage([10, 30], [10, 30]), None);
        assert_eq!(cpu_usage([10, 30], [9, 60]), None);
        assert_eq!(cpu_usage([10, 30], [50, 60]), None);
    }

    #[test]
    fn cpu_times_come_from_the_first_line_of_proc_stat() {
        let stat = "cpu  100 5 50 800 40 3 2 0 7 0\ncpu0 1 2 3 4 5 6 7 8 9 10\n";
        assert_eq!(parse_cpu_times(stat), Some([840, 1000]));
        // An old kernel's four fields are enough; anything else is not a reading.
        assert_eq!(parse_cpu_times("cpu 1 2 3 4\n"), Some([4, 10]));
        assert_eq!(parse_cpu_times("cpu 1 2 3\n"), None);
        assert_eq!(parse_cpu_times("cpu0 1 2 3 4\n"), None);
        assert_eq!(parse_cpu_times("cpu 1 x 3 4\n"), None);
        assert_eq!(parse_cpu_times(""), None);
    }

    #[test]
    fn a_sample_has_the_selected_metrics_only() {
        let mut all = Collector::new(&[]);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let sample = all.sample();
        assert!(
            sample
                .cpu
                .values()
                .all(|value| (0. ..=100.).contains(value))
        );
        let mut gpu_only = Collector::new(&["gpu_usage_percent".to_owned()]);
        let sample = gpu_only.sample();
        assert!(sample.cpu.is_empty());
        assert!(
            sample
                .gpus
                .iter()
                .all(|gpu| gpu.metrics.keys().all(|name| name == "gpu_usage_percent"))
        );
        let mut cpu_only = Collector::new(&["cpu_usage_percent".to_owned()]);
        assert!(cpu_only.sample().gpus.is_empty());
    }

    /// amdgpu's files as an RX 7800 XT (Linux 7.3) publishes them, read in the Windows
    /// host's names and units.
    #[test]
    fn amdgpu_files_become_the_windows_metrics() {
        let device = std::env::temp_dir().join(format!("rubylight-amdgpu-{}", std::process::id()));
        let hwmon = device.join("hwmon/hwmon3");
        std::fs::create_dir_all(&hwmon).unwrap();
        for (path, value) in [
            ("gpu_busy_percent", "37"),
            ("mem_info_vram_used", "992178176"),
            ("mem_info_vram_total", "17163091968"),
            ("hwmon/hwmon3/temp1_label", "edge"),
            ("hwmon/hwmon3/temp1_input", "40000"),
            ("hwmon/hwmon3/temp2_label", "junction"),
            ("hwmon/hwmon3/temp2_input", "45000"),
            ("hwmon/hwmon3/temp3_label", "mem"),
            ("hwmon/hwmon3/temp3_input", "66000"),
            ("hwmon/hwmon3/freq1_label", "sclk"),
            ("hwmon/hwmon3/freq1_input", "2124000000"),
            ("hwmon/hwmon3/freq2_label", "mclk"),
            ("hwmon/hwmon3/freq2_input", "1218000000"),
            ("hwmon/hwmon3/in0_label", "vddgfx"),
            ("hwmon/hwmon3/in0_input", "710"),
            ("hwmon/hwmon3/power1_average", "28000000"),
            ("hwmon/hwmon3/fan1_input", "1200"),
            ("hwmon/hwmon3/pwm1", "51"),
            ("hwmon/hwmon3/pwm1_max", "255"),
        ] {
            std::fs::write(device.join(path), format!("{value}\n")).unwrap();
        }
        let metrics = amd_metrics(&device, &[]);
        let _ = std::fs::remove_dir_all(&device);
        let want = [
            ("gpu_usage_percent", 37.),
            ("gpu_vram_used_mb", 992178176. / 1_048_576.),
            ("gpu_vram_usage_percent", 100. * 992178176. / 17163091968.),
            ("gpu_temperature_c", 40.),
            ("gpu_hotspot_temperature_c", 45.),
            ("gpu_memory_temperature_c", 66.),
            ("gpu_clock_mhz", 2124.),
            ("gpu_memory_clock_mhz", 1218.),
            ("gpu_voltage_mv", 710.),
            ("gpu_power_w", 28.),
            ("gpu_fan_rpm", 1200.),
            ("gpu_fan_percent", 20.),
        ];
        assert_eq!(
            metrics,
            want.iter()
                .map(|(name, value)| ((*name).to_owned(), *value))
                .collect()
        );
        // Only what is asked for, and a missing file is a missing metric.
        let picked = amd_metrics(&device, &["gpu_power_w".to_owned()]);
        assert!(picked.is_empty(), "the folder is gone: {picked:?}");
    }

    /// This PC's GPUs, when the driver is amdgpu: every reading in range.
    #[test]
    fn this_pcs_amd_gpus_are_read() {
        let mut collector = Collector::new(&[]);
        for gpu in collector.sample().gpus {
            eprintln!("{} ({}): {:?}", gpu.name, gpu.id, gpu.metrics);
            let usage = gpu.metrics.get("gpu_usage_percent").copied().unwrap();
            assert!((0. ..=100.).contains(&usage));
            assert!(gpu.metrics.get("gpu_vram_used_mb").copied().unwrap() > 0.);
        }
    }
}
