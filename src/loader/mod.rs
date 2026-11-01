//! Background image loading with a priority queue, de-duplication, and stale
//! job cancellation.

pub mod decoder;

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{
    AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering as AtomicOrdering,
};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use winit::event_loop::EventLoopProxy;

use crate::app::UserEvent;
use crate::loader::decoder::{FullImage, Thumbnail};

/// Full images more than this many entries away from the current one are
/// considered stale and skipped before decoding.
const STALE_FULL_RADIUS: usize = 5;
/// Thumbnails are useful across a wider window because the filmstrip can show
/// dozens of images at once.
const STALE_THUMB_RADIUS: usize = 48;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JobKind {
    Full,
    Thumb,
}

#[derive(Debug)]
pub enum LoaderEvent {
    Full(FullImage),
    Thumb(Thumbnail),
    Error {
        path: PathBuf,
        index: usize,
        kind: JobKind,
        message: String,
    },
    Stale {
        path: PathBuf,
        index: usize,
        kind: JobKind,
    },
}

struct Job {
    path: PathBuf,
    index: usize,
    kind: JobKind,
    priority: u8,
    sequence: u64,
}

impl PartialEq for Job {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.sequence == other.sequence
    }
}

impl Eq for Job {}

impl PartialOrd for Job {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Job {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher priority first. For equal priorities, lower sequence numbers
        // (older requests) come first.
        self.priority
            .cmp(&other.priority)
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

struct JobQueue {
    heap: Mutex<BinaryHeap<Job>>,
    available: Condvar,
    shutdown: AtomicBool,
}

impl JobQueue {
    fn new() -> Self {
        Self {
            heap: Mutex::new(BinaryHeap::new()),
            available: Condvar::new(),
            shutdown: AtomicBool::new(false),
        }
    }

    fn push(&self, job: Job) {
        if self.shutdown.load(AtomicOrdering::SeqCst) {
            return;
        }
        self.heap.lock().unwrap().push(job);
        self.available.notify_one();
    }

    fn pop(&self) -> Option<Job> {
        let mut heap = self.heap.lock().unwrap();
        loop {
            if self.shutdown.load(AtomicOrdering::SeqCst) {
                return None;
            }
            if let Some(job) = heap.pop() {
                return Some(job);
            }
            heap = self.available.wait(heap).unwrap();
        }
    }

    fn shutdown(&self) {
        self.shutdown.store(true, AtomicOrdering::SeqCst);
        self.available.notify_all();
    }
}

struct Shared {
    queue: Arc<JobQueue>,
    in_flight: Mutex<HashSet<(PathBuf, JobKind)>>,
    current_index: AtomicUsize,
    sequence: AtomicU64,
    /// Updated once the real GPU limit is known. The loader starts before the
    /// renderer so the first decode can overlap adapter/device creation.
    max_texture_dim: AtomicU32,
}

pub struct Loader {
    shared: Arc<Shared>,
    /// Decoded results are delivered through this channel. The event-loop
    /// proxy is only a wake-up signal, so results that finish before the GPU
    /// is ready can be drained synchronously at startup.
    receiver: Receiver<LoaderEvent>,
    threads: Vec<JoinHandle<()>>,
}

impl Loader {
    pub fn new(proxy: EventLoopProxy<UserEvent>, max_texture_dim: u32) -> Self {
        let shared = Arc::new(Shared {
            queue: Arc::new(JobQueue::new()),
            in_flight: Mutex::new(HashSet::new()),
            current_index: AtomicUsize::new(0),
            sequence: AtomicU64::new(0),
            max_texture_dim: AtomicU32::new(max_texture_dim.max(1)),
        });

        let worker_count = std::thread::available_parallelism()
            .map(|count| (count.get().saturating_sub(1)).clamp(1, 4))
            .unwrap_or(2);

        let (sender, receiver) = mpsc::channel();
        let mut threads = Vec::with_capacity(worker_count);
        for worker_index in 0..worker_count {
            let shared = Arc::clone(&shared);
            let proxy = proxy.clone();
            let sender = sender.clone();
            let handle = std::thread::Builder::new()
                .name(format!("picasa-loader-{worker_index}"))
                .spawn(move || worker_loop(shared, proxy, sender))
                .expect("failed to spawn image loader thread");
            threads.push(handle);
        }

        log::info!("started {worker_count} image loader threads");

        Self {
            shared,
            receiver,
            threads,
        }
    }

    /// Take every result that has finished decoding so far. Called from the
    /// event loop, including once during startup before the window is shown.
    pub fn drain_events(&self) -> Vec<LoaderEvent> {
        let mut events = Vec::new();
        while let Ok(event) = self.receiver.try_recv() {
            events.push(event);
        }
        events
    }

    pub fn set_current(&self, index: usize) {
        self.shared
            .current_index
            .store(index, AtomicOrdering::Relaxed);
    }

    /// Apply the renderer's real texture limit after GPU initialization.
    pub fn set_max_texture_dim(&self, max_texture_dim: u32) {
        self.shared
            .max_texture_dim
            .store(max_texture_dim.max(1), AtomicOrdering::Relaxed);
    }

    pub fn request_full(&self, path: &Path, index: usize, priority: u8) {
        self.request(path, index, JobKind::Full, priority);
    }

    pub fn request_thumb(&self, path: &Path, index: usize, priority: u8) {
        self.request(path, index, JobKind::Thumb, priority);
    }

    fn request(&self, path: &Path, index: usize, kind: JobKind, priority: u8) {
        let key = (path.to_path_buf(), kind);
        {
            let mut in_flight = self.shared.in_flight.lock().unwrap();
            if !in_flight.insert(key) {
                return;
            }
        }

        let sequence = self.shared.sequence.fetch_add(1, AtomicOrdering::Relaxed);
        self.shared.queue.push(Job {
            path: path.to_path_buf(),
            index,
            kind,
            priority,
            sequence,
        });
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        self.shared.queue.shutdown();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

fn worker_loop(shared: Arc<Shared>, proxy: EventLoopProxy<UserEvent>, sender: Sender<LoaderEvent>) {
    while let Some(job) = shared.queue.pop() {
        let current = shared.current_index.load(AtomicOrdering::Relaxed);
        let stale_radius = match job.kind {
            JobKind::Full => STALE_FULL_RADIUS,
            JobKind::Thumb => STALE_THUMB_RADIUS,
        };

        if job.index.abs_diff(current) > stale_radius {
            shared
                .in_flight
                .lock()
                .unwrap()
                .remove(&(job.path.clone(), job.kind));
            let _ = sender.send(LoaderEvent::Stale {
                path: job.path,
                index: job.index,
                kind: job.kind,
            });
            let _ = proxy.send_event(UserEvent::LoaderReady);
            continue;
        }

        let result = match job.kind {
            JobKind::Full => {
                let max_texture_dim = shared.max_texture_dim.load(AtomicOrdering::Relaxed);
                decoder::decode_full(&job.path, job.index, max_texture_dim).map(LoaderEvent::Full)
            }
            JobKind::Thumb => {
                decoder::decode_thumbnail(&job.path, job.index).map(LoaderEvent::Thumb)
            }
        };

        let event = match result {
            Ok(event) => event,
            Err(error) => {
                log::warn!(
                    "failed to load {} ({:?}): {error:#}",
                    job.path.display(),
                    job.kind
                );
                LoaderEvent::Error {
                    path: job.path.clone(),
                    index: job.index,
                    kind: job.kind,
                    message: format!("{error:#}"),
                }
            }
        };

        let _ = sender.send(event);
        let _ = proxy.send_event(UserEvent::LoaderReady);
        shared
            .in_flight
            .lock()
            .unwrap()
            .remove(&(job.path, job.kind));
    }
}
