//! Where the virtual pads' background threads leave what the client must be told: rumble,
//! lights and trigger effects a game wrote, and requests for the pad's motion sensors. The
//! gamepad thread's owner collects them with [`Sink::take`]; the control stream's socket, when
//! one was given, is woken at each so it does not wait for its next poll.

use super::PadReport;
use std::{
    net::UdpSocket,
    sync::{Arc, Mutex, PoisonError},
};

/// A queue of [`PadReport`]s with the socket to wake when one is added.
#[derive(Default)]
pub(super) struct Sink {
    reports: Mutex<Vec<PadReport>>,
    waker: Mutex<Option<Arc<UdpSocket>>>,
}

impl Sink {
    /// Adds a report and wakes the control stream.
    pub(super) fn push(&self, report: PadReport) {
        self.reports
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(report);
        if let Some(waker) = self
            .waker
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            // A full socket buffer already means a wake-up is pending.
            let _ = waker.send(&[1]);
        }
    }
    /// The reports added since the last call, oldest first.
    pub(super) fn take(&self) -> Vec<PadReport> {
        std::mem::take(&mut *self.reports.lock().unwrap_or_else(PoisonError::into_inner))
    }
    /// The socket to wake from now on.
    pub(super) fn set_waker(&self, waker: Arc<UdpSocket>) {
        *self.waker.lock().unwrap_or_else(PoisonError::into_inner) = Some(waker);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_come_out_in_order_once() {
        let sink = Sink::default();
        sink.push(PadReport::Motion {
            id: 0,
            capabilities: 0x30,
        });
        sink.push(PadReport::Feedback {
            id: 1,
            kind: 0x010b,
            data: vec![1, 2, 3, 4],
        });
        let reports = sink.take();
        assert_eq!(reports.len(), 2);
        assert!(matches!(reports[0], PadReport::Motion { id: 0, .. }));
        assert!(sink.take().is_empty());
    }

    #[test]
    fn a_report_wakes_the_socket() {
        let server = UdpSocket::bind("127.0.0.1:0").unwrap();
        let waker = UdpSocket::bind("127.0.0.1:0").unwrap();
        waker.connect(server.local_addr().unwrap()).unwrap();
        let sink = Sink::default();
        sink.set_waker(Arc::new(waker));
        sink.push(PadReport::Motion {
            id: 0,
            capabilities: 0,
        });
        server
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let mut byte = [0u8; 4];
        assert_eq!(server.recv(&mut byte).unwrap(), 1);
    }
}
