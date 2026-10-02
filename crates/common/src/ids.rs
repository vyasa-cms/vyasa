//! Snowflake-style unique ID generation.
//!
//! Layout: 41 bits timestamp (ms since 2024-01-01) | 10 bits worker |
//! 12 bits sequence. Ids are strictly increasing within a process, which
//! makes them usable as both primary keys and cursor tokens.

use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Custom epoch: 2024-01-01T00:00:00Z in unix milliseconds.
const EPOCH_MS: u64 = 1_704_067_200_000;
/// Bits allocated to the worker id.
const WORKER_BITS: u32 = 10;
/// Bits allocated to the per-millisecond sequence.
const SEQ_BITS: u32 = 12;
/// Maximum worker id (inclusive).
const MAX_WORKER: u16 = (1 << WORKER_BITS) - 1;
/// Maximum sequence value within one millisecond.
const MAX_SEQ: u64 = (1 << SEQ_BITS) - 1;

/// A validated worker identifier (0..=1023).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkerId(u16);

impl WorkerId {
    /// The zero worker id.
    pub const ZERO: Self = Self(0);

    /// Creates a worker id if `id` is within the valid range.
    #[must_use]
    pub fn new(id: u16) -> Option<Self> {
        (id <= MAX_WORKER).then_some(Self(id))
    }

    /// Returns the raw numeric worker id.
    #[must_use]
    pub fn value(self) -> u16 {
        self.0
    }
}

#[derive(Debug, Default)]
struct IdState {
    last_ms: u64,
    seq: u64,
}

/// Snowflake-style ID generator.
///
/// Generated ids are strictly increasing within this instance. If the
/// system clock regresses, generation continues from the last observed
/// millisecond so ordering and uniqueness are preserved.
pub struct IdGen {
    worker: WorkerId,
    state: Mutex<IdState>,
}

impl IdGen {
    /// Creates a generator for `worker`.
    #[must_use]
    pub fn new(worker: WorkerId) -> Self {
        Self {
            worker,
            state: Mutex::new(IdState::default()),
        }
    }

    /// Generates the next unique id.
    ///
    /// Never blocks: when the sequence for the current millisecond is
    /// exhausted, the internal timestamp advances artificially by one
    /// millisecond (documented trade-off: ids may run ahead of the wall
    /// clock under extreme generation rates).
    #[must_use]
    pub fn next(&self) -> u64 {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = unix_ms();
        if now > state.last_ms {
            state.last_ms = now;
            state.seq = 0;
        } else if state.seq < MAX_SEQ {
            state.seq += 1;
        } else {
            state.last_ms += 1;
            state.seq = 0;
        }
        ((state.last_ms.saturating_sub(EPOCH_MS)) << (WORKER_BITS + SEQ_BITS))
            | (u64::from(self.worker.0) << SEQ_BITS)
            | state.seq
    }
}

/// Returns the process-wide default generator.
///
/// Its worker id comes from `VYASA_ID_WORKER` (default 0). Use distinct
/// worker ids when running multiple processes against the same database.
#[must_use]
pub fn global() -> &'static IdGen {
    static GLOBAL: OnceLock<IdGen> = OnceLock::new();
    GLOBAL.get_or_init(|| {
        let requested = std::env::var("VYASA_ID_WORKER")
            .ok()
            .and_then(|v| v.parse::<u16>().ok())
            .unwrap_or(0)
            .min(MAX_WORKER);
        let worker = WorkerId::new(requested).unwrap_or(WorkerId::ZERO);
        IdGen::new(worker)
    })
}

/// Generates an id from the process-wide default generator.
#[must_use]
pub fn next_id() -> u64 {
    global().next()
}

/// Generates an id (as `i64`, the database column type) from the
/// process-wide default generator. Snowflake ids use at most 63 bits, so
/// the conversion cannot overflow in practice; saturates defensively.
#[must_use]
pub fn next_id_i64() -> i64 {
    i64::try_from(next_id()).unwrap_or(i64::MAX)
}

/// Current unix time in milliseconds; 0 if the clock is before 1970.
fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::{next_id, IdGen, WorkerId};
    use std::collections::HashSet;

    #[test]
    fn worker_id_bounds() {
        assert_eq!(WorkerId::new(0).map(WorkerId::value), Some(0));
        assert_eq!(WorkerId::new(1023).map(WorkerId::value), Some(1023));
        assert_eq!(WorkerId::new(1024), None);
        assert_eq!(WorkerId::new(u16::MAX), None);
    }

    #[test]
    fn ids_are_unique_and_monotonic() {
        let gen = IdGen::new(WorkerId::ZERO);
        let mut seen = HashSet::new();
        let mut last = 0_u64;
        for _ in 0..10_000 {
            let id = gen.next();
            assert!(id > last, "id {id} not greater than {last}");
            assert!(seen.insert(id), "duplicate id {id}");
            last = id;
        }
    }

    #[test]
    fn worker_bits_are_embedded() {
        let gen = IdGen::new(WorkerId::new(7).unwrap());
        let id = gen.next();
        assert_eq!(((id >> 12) & 0x3ff) as u16, 7);
    }

    #[test]
    fn ids_fit_in_i64_and_are_epoch_based() {
        let gen = IdGen::new(WorkerId::ZERO);
        let id = gen.next();
        assert!(i64::try_from(id).is_ok());
        // Timestamp bits are far in the future (year 2100+ would still fit).
        assert!(id >> 22 > 0);
    }

    #[test]
    fn concurrent_generation_is_unique() {
        let gen = std::sync::Arc::new(IdGen::new(WorkerId::ZERO));
        let (tx, rx) = std::sync::mpsc::channel();
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let gen = std::sync::Arc::clone(&gen);
                let tx = tx.clone();
                std::thread::spawn(move || {
                    for _ in 0..2500 {
                        let _ = tx.send(gen.next());
                    }
                })
            })
            .collect();
        drop(tx);
        let mut seen = HashSet::new();
        for id in rx {
            assert!(seen.insert(id), "duplicate id {id}");
        }
        for t in threads {
            t.join().expect("worker thread panicked");
        }
        assert_eq!(seen.len(), 10_000);
    }

    #[test]
    fn global_generator_works() {
        let a = next_id();
        let b = next_id();
        assert_ne!(a, b);
    }
}
