use crate::state::{Host, Shared};
use axum::http::StatusCode;
use butterpollo_session::session::Role;
use butterpollo_win_graphics::telemetry::Collector;
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Weak,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

type Error = (StatusCode, &'static str);
/// Packed `(unix_ms << 8) | (gpu_busy_percent + 1)` from the latest telemetry worker sample;
/// `0` when no sample has recorded `gpu_usage_percent`.
static LAST_GPU_BUSY: AtomicU64 = AtomicU64::new(0);
const GPU_BUSY_MAX_AGE_MS: u64 = 15_000;

pub struct Request {
    client: String,
    internal_gpu_only: bool,
    reply: Option<tokio::sync::oneshot::Sender<Result<Value, Error>>>,
}

fn unix_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn max_gpu_usage_percent(value: &Value) -> Option<u8> {
    value["gpus"]
        .as_array()?
        .iter()
        .filter_map(|gpu| gpu["metrics"]["gpu_usage_percent"].as_f64())
        .filter(|pct| pct.is_finite() && *pct >= 0.0)
        .map(|pct| pct.round().clamp(0.0, 100.0) as u8)
        .max()
}

fn pack_gpu_busy(unix_ms: u64, pct: u8) -> u64 {
    (unix_ms << 8) | u64::from(pct.min(100) + 1)
}

fn unpack_gpu_busy_at(packed: u64, now_unix_ms: u64) -> Option<u8> {
    let raw = (packed & 0xFF) as u8;
    if raw == 0 {
        return None;
    }
    let at_ms = packed >> 8;
    (now_unix_ms.saturating_sub(at_ms) <= GPU_BUSY_MAX_AGE_MS).then_some(raw - 1)
}

/// Non-blocking: triggers an off-thread `gpu_usage_percent` sample on the telemetry worker for
/// `client` and returns the freshest recent reading (within 15 s), if any.
pub fn poll_and_read_gpu_busy(h: &Shared, client: &str) -> Option<u8> {
    if let Ok(mut worker) = h.hardware_metrics.try_lock() {
        let req = || Request {
            client: client.to_owned(),
            internal_gpu_only: true,
            reply: None,
        };
        let sent = if let Some(sender) = worker.as_ref() {
            sender.try_send(req())
        } else if let Ok(sender) = spawn_worker(h) {
            *worker = Some(sender.clone());
            sender.try_send(req())
        } else {
            Ok(())
        };
        if matches!(sent, Err(mpsc::TrySendError::Disconnected(_))) {
            *worker = spawn_worker(h).ok();
            if let Some(sender) = worker.as_ref() {
                let _ = sender.try_send(req());
            }
        }
    }
    unpack_gpu_busy_at(LAST_GPU_BUSY.load(Ordering::Acquire), unix_ms_now())
}

fn options(h: &Host, client: &str) -> Result<(Duration, Vec<String>), Error> {
    let config = h.config.read().unwrap();
    if !config.hardware_metrics() {
        return Err((StatusCode::FORBIDDEN, "Hardware metrics are disabled"));
    }
    let options = (
        Duration::from_millis(config.hardware_metrics_interval()),
        config.list("hardware_metrics_selection"),
    );
    drop(config);
    if h.stop.load(Ordering::Acquire)
        || !h.sessions.lock().unwrap().active.values().any(|session| {
            session.launch.client.uuid == client
                && session.launch.role != Role::InputOnly
                && !session.stopping()
        })
    {
        return Err((StatusCode::CONFLICT, "An active video stream is required"));
    }
    Ok(options)
}

fn spawn_worker(h: &Shared) -> Result<mpsc::SyncSender<Request>, Error> {
    let (sender, receiver) = mpsc::sync_channel(4);
    let host = Arc::downgrade(h);
    std::thread::Builder::new()
        .name("hardware-metrics".into())
        .spawn(move || run(host, receiver))
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "Hardware metrics are unavailable",
            )
        })?;
    Ok(sender)
}

pub async fn sample(h: &Shared, client: String) -> Result<Value, Error> {
    let (_, selected) = options(h, &client)?;
    let (reply, response) = tokio::sync::oneshot::channel();
    {
        let mut worker = h.hardware_metrics.lock().unwrap();
        let request = Request {
            client: client.clone(),
            internal_gpu_only: false,
            reply: Some(reply),
        };
        let send = if let Some(sender) = worker.as_ref() {
            sender.try_send(request)
        } else {
            let sender = spawn_worker(h)?;
            *worker = Some(sender.clone());
            sender.try_send(request)
        };
        match send {
            Ok(()) => {}
            Err(mpsc::TrySendError::Disconnected(request)) => {
                *worker = None;
                let sender = spawn_worker(h)?;
                *worker = Some(sender.clone());
                sender
                    .try_send(request)
                    .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "Hardware metrics are busy"))?;
            }
            Err(mpsc::TrySendError::Full(_)) => {
                return Err((StatusCode::SERVICE_UNAVAILABLE, "Hardware metrics are busy"));
            }
        }
    }
    let value = tokio::time::timeout(Duration::from_secs(2), response)
        .await
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "Hardware metrics timed out",
            )
        })?
        .map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "Hardware metrics are unavailable",
            )
        })??;
    // A driver call may outlive its stream or a console setting change.
    if options(h, &client)?.1 != selected {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "Hardware metric selection changed",
        ));
    }
    Ok(value)
}

fn active_video_stream(h: &Host, client: &str) -> bool {
    !h.stop.load(Ordering::Acquire)
        && h.sessions.lock().unwrap().active.values().any(|session| {
            session.launch.client.uuid == client
                && session.launch.role != Role::InputOnly
                && !session.stopping()
        })
}

fn run(host: Weak<Host>, requests: mpsc::Receiver<Request>) {
    let mut collector = None;
    let mut selected = Vec::new();
    let mut cached: Option<(Instant, Value)> = None;
    let mut requester = String::new();
    let mut internal_requester = false;
    loop {
        let received = if collector.is_some() {
            requests.recv_timeout(Duration::from_millis(250))
        } else {
            requests
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        };
        let request = match received {
            Ok(request) => Some(request),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        let Some(h) = host.upgrade() else { return };
        if h.stop.load(Ordering::Acquire) {
            return;
        }
        let Some(request) = request else {
            // No background polling: the client or queue-clamp caller must keep requesting samples.
            let still_valid = if internal_requester {
                active_video_stream(&h, &requester) && butterpollo_win_io::limiter::queue_clamping()
            } else {
                options(&h, &requester).is_ok()
            };
            if !still_valid {
                collector = None;
                cached = None;
                LAST_GPU_BUSY.store(0, Ordering::Release);
            }
            continue;
        };
        if request.reply.as_ref().is_some_and(|r| r.is_closed()) {
            continue;
        }
        let (interval, mut metrics) = if request.internal_gpu_only {
            if !active_video_stream(&h, &request.client) {
                continue;
            }
            match options(&h, &request.client) {
                Ok((interval, mut metrics)) => {
                    if !metrics.is_empty() && !metrics.iter().any(|m| m == "gpu_usage_percent") {
                        metrics.push("gpu_usage_percent".into());
                    }
                    (interval, metrics)
                }
                Err(_) => (Duration::from_secs(1), vec!["gpu_usage_percent".into()]),
            }
        } else {
            match options(&h, &request.client) {
                Ok(options) => options,
                Err(error) => {
                    if let Some(reply) = request.reply {
                        let _ = reply.send(Err(error));
                    }
                    continue;
                }
            }
        };
        if butterpollo_win_io::limiter::queue_clamping()
            && !metrics.is_empty()
            && !metrics.iter().any(|m| m == "gpu_usage_percent")
        {
            metrics.push("gpu_usage_percent".into());
        }
        internal_requester = request.internal_gpu_only;
        requester = request.client;
        if collector.is_none() || selected != metrics {
            // ADLX has one process-wide lifetime; release it before reopening.
            drop(collector.take());
            collector = Some(Collector::new(&metrics));
            selected = metrics;
            cached = None;
        }
        if cached
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() >= interval)
        {
            let sample = collector.as_mut().unwrap().sample();
            let now_ms = unix_ms_now();
            let mut value = serde_json::to_value(sample).unwrap();
            value["version"] = json!(1);
            value["sampled_at_unix_ms"] = json!(now_ms);
            if let Some(busy) = max_gpu_usage_percent(&value) {
                LAST_GPU_BUSY.store(pack_gpu_busy(now_ms, busy), Ordering::Release);
            }
            cached = Some((Instant::now(), value));
        }
        if let Some(reply) = request.reply {
            let mut value = cached.as_ref().unwrap().1.clone();
            value["interval_ms"] = json!(interval.as_millis() as u64);
            let _ = reply.send(Ok(value));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_support::Fixture;

    #[test]
    fn gpu_busy_pack_and_max_extraction_bound_age_and_percent() {
        let val = json!({
            "gpus": [
                { "metrics": { "gpu_usage_percent": 42.2 } },
                { "metrics": { "gpu_usage_percent": 98.6 } }
            ]
        });
        assert_eq!(max_gpu_usage_percent(&val), Some(99));
        let packed = pack_gpu_busy(100_000, 99);
        assert_eq!(unpack_gpu_busy_at(packed, 105_000), Some(99));
        assert_eq!(unpack_gpu_busy_at(packed, 115_000), Some(99));
        assert_eq!(unpack_gpu_busy_at(packed, 115_001), None);
        assert_eq!(unpack_gpu_busy_at(0, 100_000), None);
    }

    #[test]
    fn metrics_require_opt_in_and_the_requesting_clients_active_video_stream() {
        let f = Fixture::new();
        let client = f.client(0x0200_0000);
        f.host
            .config
            .write()
            .unwrap()
            .values
            .insert("hardware_metrics".into(), "false".into());
        assert_eq!(
            options(&f.host, &client.uuid).unwrap_err().0,
            StatusCode::FORBIDDEN
        );
        f.host
            .config
            .write()
            .unwrap()
            .values
            .insert("hardware_metrics".into(), "true".into());
        assert_eq!(
            options(&f.host, &client.uuid).unwrap_err().0,
            StatusCode::CONFLICT
        );
        let input = f.session(client.clone(), Role::InputOnly);
        assert!(options(&f.host, &client.uuid).is_err());
        let stream = f.session(client.clone(), Role::Stream);
        assert!(options(&f.host, "another-client").is_err());
        assert_eq!(
            options(&f.host, &client.uuid).unwrap().0,
            Duration::from_secs(1)
        );
        stream.stop();
        input.stop();
        assert!(options(&f.host, &client.uuid).is_err());
        assert!(f.host.hardware_metrics.lock().unwrap().is_none());
    }
}
