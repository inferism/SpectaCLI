//! Per-agent output path: every byte goes into the scrollback ring, and while
//! a pane is attached it is also coalesced into one message per `WINDOW`
//! (PLAN.md 2.2), so many streaming panes don't flood IPC.

use super::ring::Ring;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Coalescing window: output arriving within it is sent as one message.
pub const WINDOW: Duration = Duration::from_millis(12);

/// Delivers one batch to the attached pane. Returns false when the receiver
/// is gone (e.g. the webview reloaded), which detaches it.
pub type Sink = Box<dyn Fn(Vec<u8>) -> bool + Send>;

struct State {
    ring: Ring,
    pending: Vec<u8>,
    sink: Option<Sink>,
    closed: bool,
}

pub struct Output {
    state: Mutex<State>,
    cv: Condvar,
}

impl Output {
    pub fn new(scrollback: usize) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(State { ring: Ring::new(scrollback), pending: Vec::new(), sink: None, closed: false }),
            cv: Condvar::new(),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Called by the PTY reader thread.
    pub fn push(&self, data: &[u8]) {
        let mut s = self.lock();
        s.ring.push(data);
        if s.sink.is_some() {
            let wake = s.pending.is_empty();
            s.pending.extend_from_slice(data);
            if wake {
                self.cv.notify_one();
            }
        }
    }

    /// Route live output to `sink` and return the scrollback to replay first.
    /// Atomic with `push`: every byte is either in the returned snapshot or
    /// sent to `sink`, never both or neither. Replaces any previous sink.
    pub fn attach(&self, sink: Sink) -> Vec<u8> {
        let mut s = self.lock();
        s.pending.clear();
        s.sink = Some(sink);
        s.ring.snapshot()
    }

    pub fn detach(&self) {
        let mut s = self.lock();
        s.sink = None;
        s.pending.clear();
    }

    /// The PTY reached EOF: flush what is pending, then stop the flusher.
    pub fn close(&self) {
        self.lock().closed = true;
        self.cv.notify_one();
    }

    /// Flusher loop; run it on its own thread. Sleeps until output arrives,
    /// waits one `WINDOW` to coalesce, then sends. Idle cost is zero.
    pub fn run_flusher(&self, window: Duration) {
        loop {
            {
                let mut s = self.lock();
                while s.pending.is_empty() && !s.closed {
                    s = self.cv.wait(s).unwrap_or_else(|e| e.into_inner());
                }
                if s.pending.is_empty() {
                    return; // closed and drained
                }
            }
            std::thread::sleep(window);
            let mut s = self.lock();
            let data = std::mem::take(&mut s.pending);
            // Sent under the lock so an `attach` can't slip in between taking
            // the batch and delivering it to the old sink.
            if !data.is_empty() && s.sink.as_ref().is_some_and(|sink| !sink(data)) {
                s.sink = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn channel_sink() -> (Sink, mpsc::Receiver<Vec<u8>>) {
        let (tx, rx) = mpsc::channel();
        (Box::new(move |d| tx.send(d).is_ok()), rx)
    }

    fn spawn_flusher(out: &Arc<Output>) -> std::thread::JoinHandle<()> {
        let out = out.clone();
        std::thread::spawn(move || out.run_flusher(Duration::from_millis(30)))
    }

    #[test]
    fn coalesces_bursts_into_one_message() {
        let out = Output::new(1024);
        let (sink, rx) = channel_sink();
        assert!(out.attach(sink).is_empty());
        let t = spawn_flusher(&out);
        out.push(b"a");
        out.push(b"b");
        out.push(b"c");
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), b"abc");
        out.close();
        t.join().unwrap();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn attach_replays_scrollback_without_duplicates() {
        let out = Output::new(1024);
        out.push(b"before ");
        let (sink, rx) = channel_sink();
        assert_eq!(out.attach(sink), b"before ");
        let t = spawn_flusher(&out);
        out.push(b"after");
        assert_eq!(rx.recv_timeout(Duration::from_secs(2)).unwrap(), b"after");
        // A re-attach gets everything in the snapshot and nothing pending.
        let (sink2, rx2) = channel_sink();
        assert_eq!(out.attach(sink2), b"before after");
        out.close();
        t.join().unwrap();
        assert!(rx2.try_recv().is_err());
    }

    #[test]
    fn dead_sink_detaches_and_close_flushes() {
        let out = Output::new(1024);
        out.attach(Box::new(|_| false));
        let t = spawn_flusher(&out);
        out.push(b"x");
        std::thread::sleep(Duration::from_millis(100));
        assert!(out.lock().sink.is_none());
        out.push(b"y"); // not queued: nobody is attached
        assert!(out.lock().pending.is_empty());
        out.close();
        t.join().unwrap();
        assert_eq!(out.lock().ring.snapshot(), b"xy");
    }
}
