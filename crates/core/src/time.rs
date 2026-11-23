use std::time::Instant;

/// Monotonic microsecond clock shared across video, audio, recording, and streaming pipelines.
#[derive(Debug, Clone)]
pub struct Clock {
    origin: Instant,
}

impl Clock {
    pub fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }

    /// Returns elapsed microseconds since engine startup.
    pub fn now_us(&self) -> u64 {
        self.origin.elapsed().as_micros() as u64
    }

    /// Returns elapsed nanoseconds since engine startup.
    pub fn now_ns(&self) -> u64 {
        self.origin.elapsed().as_nanos() as u64
    }

    /// Returns elapsed milliseconds since engine startup.
    pub fn now_ms(&self) -> u64 {
        self.origin.elapsed().as_millis() as u64
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}
