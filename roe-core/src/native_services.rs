use std::time::Instant;

/// Monotonic time used by editor and watcher policy.
pub trait Clock: Send + Sync {
    fn now(&self) -> Instant;
}

/// Thread-safe notification boundary used by native services to wake a
/// frontend without knowing whether it is terminal, Winit, or headless.
pub trait FrontendWake: Send + Sync {
    fn wake(&self);
}

#[derive(Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}
