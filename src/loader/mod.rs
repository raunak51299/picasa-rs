//! Background image loading with a priority queue, de-duplication, and stale
//! job cancellation.

pub mod decoder;

use std::any::Any;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{
    AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering as AtomicOrdering,
};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
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
    /// Decode every frame of the current animated GIF/WebP.
    Animation,
}

#[derive(Debug)]
pub enum LoaderEvent {
    Full(FullImage),
    Thumb(Thumbnail),
    Animation(decoder::Animation),
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

/// Lock a mutex even if a previous holder panicked. A poisoned queue or
/// in-flight set is still structurally valid, and refusing to use it would
/// stop every later image from loading.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A job is stale when the viewer has moved further than the kind's radius.
fn is_stale(index: usize, current: usize, kind: JobKind) -> bool {
    let radius = match kind {
        JobKind::Full => STALE_FULL_RADIUS,
        JobKind::Thumb => STALE_THUMB_RADIUS,
        // Animation is only useful for the photo currently on screen.
        JobKind::Animation => 0,
    };
    index.abs_diff(current) > radius
}

/// Claim a job for decoding, returning false when it is already in flight.
fn claim_job(in_flight: &Mutex<HashSet<(PathBuf, JobKind)>>, path: &Path, kind: JobKind) -> bool {
    lock(in_flight).insert((path.to_path_buf(), kind))
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "image decoder panicked".to_string()
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
        lock(&self.heap).push(job);
        self.available.notify_one();
    }

    fn pop(&self) -> Option<Job> {
        let mut heap = lock(&self.heap);
        loop {
            if self.shutdown.load(AtomicOrdering::SeqCst) {
                return None;
            }
            if let Some(job) = heap.pop() {
                return Some(job);
            }
            heap = self
                .available
                .wait(heap)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
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

    pub fn request_animation(&self, path: &Path, index: usize, priority: u8) {
        self.request(path, index, JobKind::Animation, priority);
    }

    /// Whether a decode for this file and kind is already queued or running.
    pub fn has_in_flight(&self, path: &Path, kind: JobKind) -> bool {
        lock(&self.shared.in_flight).contains(&(path.to_path_buf(), kind))
    }

    fn request(&self, path: &Path, index: usize, kind: JobKind, priority: u8) {
        if !claim_job(&self.shared.in_flight, path, kind) {
            return;
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
        let event = if is_stale(job.index, current, job.kind) {
            LoaderEvent::Stale {
                path: job.path.clone(),
                index: job.index,
                kind: job.kind,
            }
        } else {
            decode_job(&shared, &job)
        };

        // Clear in-flight before waking the UI so a handler that re-requests
        // this file is not rejected as a duplicate.
        lock(&shared.in_flight).remove(&(job.path.clone(), job.kind));
        let _ = sender.send(event);
        let _ = proxy.send_event(UserEvent::LoaderReady);
    }
}

fn decode_job(shared: &Shared, job: &Job) -> LoaderEvent {
    let result = catch_unwind(AssertUnwindSafe(|| match job.kind {
        JobKind::Full => {
            let max_texture_dim = shared.max_texture_dim.load(AtomicOrdering::Relaxed);
            decoder::decode_full(&job.path, job.index, max_texture_dim).map(LoaderEvent::Full)
        }
        JobKind::Thumb => decoder::decode_thumbnail(&job.path, job.index).map(LoaderEvent::Thumb),
        JobKind::Animation => {
            let max_texture_dim = shared.max_texture_dim.load(AtomicOrdering::Relaxed);
            decoder::decode_animation(&job.path, job.index, max_texture_dim)
                .map(LoaderEvent::Animation)
        }
    }));

    let message = match result {
        Ok(Ok(event)) => return event,
        Ok(Err(error)) => format!("{error:#}"),
        Err(payload) => panic_message(payload),
    };

    log::warn!(
        "failed to load {} ({:?}): {message}",
        job.path.display(),
        job.kind
    );
    LoaderEvent::Error {
        path: job.path.clone(),
        index: job.index,
        kind: job.kind,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(path: &str, kind: JobKind, priority: u8, sequence: u64) -> Job {
        Job {
            path: PathBuf::from(path),
            index: 0,
            kind,
            priority,
            sequence,
        }
    }

    #[test]
    fn jobs_are_ordered_by_priority_then_age() {
        let mut heap = BinaryHeap::new();
        heap.push(job("low-old", JobKind::Full, 0, 1));
        heap.push(job("high-new", JobKind::Full, 10, 5));
        heap.push(job("high-old", JobKind::Full, 10, 2));
        heap.push(job("low-new", JobKind::Full, 0, 9));

        let order: Vec<String> = std::iter::from_fn(|| heap.pop())
            .map(|job| job.path.to_string_lossy().into_owned())
            .collect();
        assert_eq!(order, ["high-old", "high-new", "low-old", "low-new"]);
    }

    #[test]
    fn queue_pops_jobs_and_stops_after_shutdown() {
        let queue = JobQueue::new();
        queue.push(job("low", JobKind::Full, 1, 0));
        queue.push(job("high", JobKind::Thumb, 5, 1));

        assert_eq!(queue.pop().unwrap().path, PathBuf::from("high"));
        assert_eq!(queue.pop().unwrap().path, PathBuf::from("low"));

        queue.shutdown();
        assert!(queue.pop().is_none());

        queue.push(job("ignored", JobKind::Full, 0, 0));
        assert!(queue.pop().is_none());
    }

    #[test]
    fn queue_recovers_from_a_poisoned_mutex() {
        let queue = JobQueue::new();
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let _guard = queue.heap.lock().unwrap();
            panic!("poison the queue");
        }));

        queue.push(job("after-panic", JobKind::Full, 0, 0));
        assert_eq!(queue.pop().unwrap().path, PathBuf::from("after-panic"));
    }

    #[test]
    fn in_flight_jobs_are_claimed_once() {
        let in_flight = Mutex::new(HashSet::new());
        let path = Path::new("photo.jpg");

        assert!(claim_job(&in_flight, path, JobKind::Full));
        assert!(!claim_job(&in_flight, path, JobKind::Full));
        assert!(claim_job(&in_flight, path, JobKind::Thumb));

        lock(&in_flight).remove(&(path.to_path_buf(), JobKind::Full));
        assert!(claim_job(&in_flight, path, JobKind::Full));
    }

    #[test]
    fn stale_radius_depends_on_job_kind() {
        assert!(!is_stale(5, 0, JobKind::Full));
        assert!(is_stale(6, 0, JobKind::Full));
        assert!(!is_stale(0, 5, JobKind::Full));
        assert!(!is_stale(48, 0, JobKind::Thumb));
        assert!(is_stale(49, 0, JobKind::Thumb));
        assert!(!is_stale(0, 0, JobKind::Animation));
        assert!(is_stale(1, 0, JobKind::Animation));
    }

    #[test]
    fn panic_payloads_become_messages() {
        assert_eq!(panic_message(Box::new("boom")), "boom");
        assert_eq!(panic_message(Box::new(String::from("boom"))), "boom");
        assert_eq!(panic_message(Box::new(7_u8)), "image decoder panicked");
    }
}
