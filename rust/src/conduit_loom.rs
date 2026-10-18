//! Loom concurrency models (T1176 MC): exhaustive interleaving checks
//! over the crate's shared-state protocols. Runs only under the loom
//! CI job (`cargo test --lib --features loom loom`); compiled out
//! everywhere else, so coverage and normal gates never see it.
//!
//! Inventory note: `arc` is single-threaded (no `thread::spawn`, no
//! async runtime, zero `unsafe`). The modeled surface is the
//! `Mutex<SessionState>` protocol (the one lock in prod, exercised here
//! through the real `Client` methods) plus the fetch-add uniqueness
//! pattern behind `MESSAGE_COUNTER` and the test `TempDir` counters.
//! `OnceLock` cells are std-verified primitives; our hazard there was
//! test-harness multi-writer races, closed by audit (see
//! `docs/t1176/model-checking.md`), not by modeling.

use super::*;
use loom::sync::Arc;
use loom::thread;

/// Two threads hammering the session cache through the real `Client`
/// methods: Loom explores every interleaving, failing on deadlock,
/// panic, or (via the assertions) lost updates and torn pairs.
#[test]
fn loom_session_state_concurrent_access_is_sound() {
    loom::model::Builder::new().check(|| {
        let client = Arc::new(Client::new("http://127.0.0.1:9/", "t").unwrap());
        let writer = client.clone();
        let t1 = thread::spawn(move || {
            writer.set_authenticated_user("alice", "PHID-1");
            writer.enable_capabilities(&["gzip".to_string()]);
        });
        let reader = client.clone();
        let t2 = thread::spawn(move || {
            reader.set_authenticated_user("bob", "PHID-2");
            reader.enable_capabilities(&["json".to_string()]);
            let _ = reader.authenticated_user();
        });
        t1.join().unwrap();
        t2.join().unwrap();
        // Capabilities union monotonically: both survive every schedule.
        for want in ["gzip", "json"] {
            assert!(
                client
                    .state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .capabilities
                    .contains(want),
                "lost capability update for {want}"
            );
        }
        // The identity pair is written under one lock hold, so readers
        // only ever see a coherent pair (never alice/PHID-2).
        let pair = client.authenticated_user();
        assert!(
            matches!(
                pair.as_ref().map(|(u, p)| (u.as_str(), p.as_str())),
                Some(("alice", "PHID-1")) | Some(("bob", "PHID-2"))
            ),
            "torn identity pair: {pair:?}"
        );
    });
}

/// `fetch_add` hands out distinct values under contention (the
/// `MESSAGE_COUNTER` uniqueness pattern): N increments from 0 yield
/// exactly 0..N in some order, on every schedule.
#[test]
fn loom_fetch_add_values_are_unique() {
    use loom::sync::atomic::{AtomicU64, Ordering};
    loom::model::Builder::new().check(|| {
        let counter = Arc::new(AtomicU64::new(0));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let counter = counter.clone();
                thread::spawn(move || {
                    let a = counter.fetch_add(1, Ordering::Relaxed);
                    let b = counter.fetch_add(1, Ordering::Relaxed);
                    [a, b]
                })
            })
            .collect();
        let mut seen = Vec::new();
        for thread in threads {
            seen.extend(thread.join().unwrap());
        }
        seen.sort_unstable();
        assert_eq!(seen, vec![0, 1, 2, 3]);
    });
}
