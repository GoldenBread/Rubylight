//! On-demand hardware samples. The host owns polling and keeps the reads off stream threads. On Linux a sample carries the CPU usage from `/proc/stat`; GPU metrics are not available yet, so a sample lists no GPU.
use serde::Serialize;
use std::collections::BTreeMap;

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
}
impl Collector {
    /// Construct, sample and drop on the host's single telemetry worker.
    pub fn new(selected: &[String]) -> Self {
        Self {
            selected: selected.to_vec(),
            cpu: wanted(selected, "cpu_usage_percent")
                .then(cpu_times)
                .flatten(),
        }
    }
    /// The CPU usage since the last sample, when it is selected and readable. No GPU is
    /// sampled on Linux.
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
        sample
    }
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
    fn a_sample_has_the_selected_cpu_metric_and_no_gpu() {
        let mut all = Collector::new(&[]);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let sample = all.sample();
        assert!(sample.gpus.is_empty());
        assert!(
            sample
                .cpu
                .values()
                .all(|value| (0. ..=100.).contains(value))
        );
        let mut gpu_only = Collector::new(&["gpu_usage_percent".to_owned()]);
        let sample = gpu_only.sample();
        assert!(sample.cpu.is_empty() && sample.gpus.is_empty());
    }
}
