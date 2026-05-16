//! Stateful `_search?scroll=…` / `POST /_search/scroll` lifecycle.
//!
//! deces-backend bulk-match drives ES via:
//!
//! ```text
//! POST /<index>/_search?scroll=1m  body={ size:1000, query:{…} }   → _scroll_id
//! POST /_search/scroll             body={ scroll:"1m", scroll_id:"…" } → next page
//! ```
//!
//! The Surch implementation is deliberately minimal: a process-local
//! `Mutex<HashMap>` keyed by an opaque random id, lazy TTL eviction on
//! every `insert`/`take` (no background thread), and a cursor of "next
//! document offset" so each scroll fetch reuses the existing
//! `from`/`size` pagination of `run_search`. Survives ≥ 5 minutes when
//! the client re-issues `?scroll=1m` every ~50 s (cf. matchID intake
//! batch §2.9, acceptance criterion §4.5).

use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use uuid::Uuid;

use crate::search::SearchRequest;

/// Default TTL applied when the client-supplied `scroll` keepalive
/// fails to parse. Matches the matchID default (`"1m"`).
const DEFAULT_SCROLL_TTL: Duration = Duration::from_secs(60);

/// State held between successive `_search/scroll` calls for one
/// logical scan. The cursor tracks the offset of the *next* document
/// the client expects to receive.
#[derive(Debug)]
pub struct ScrollContext {
    pub index: String,
    pub request: SearchRequest,
    /// Offset of the next document to return. Starts at `from + size`
    /// once the initial `_search` page has been served.
    pub cursor: usize,
    /// Page size carried forward across `_search/scroll` calls.
    pub size: usize,
    pub expires_at: Instant,
}

/// Process-local registry of open scroll contexts.
///
/// `take` removes the entry; callers reinstall the context with a
/// fresh expiry once they have advanced the cursor. `gc_expired` is
/// invoked from both `insert` and `take` to keep the table bounded
/// without a background thread.
#[derive(Debug, Default)]
pub struct ScrollTable {
    entries: Mutex<HashMap<String, ScrollContext>>,
}

impl ScrollTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new scroll context and return its opaque scroll id.
    pub fn insert(&self, ctx: ScrollContext) -> String {
        let mut guard = self.entries.lock().expect("scroll table poisoned");
        Self::gc_locked(&mut guard);
        let id = generate_scroll_id();
        guard.insert(id.clone(), ctx);
        id
    }

    /// Remove and return a scroll context by id, if it exists and has
    /// not expired. Expired entries are dropped as a side effect.
    pub fn take(&self, id: &str) -> Option<ScrollContext> {
        let mut guard = self.entries.lock().expect("scroll table poisoned");
        Self::gc_locked(&mut guard);
        guard.remove(id)
    }

    /// Drop every expired entry. Lazy; called from `insert` and `take`.
    pub fn gc_expired(&self) {
        let mut guard = self.entries.lock().expect("scroll table poisoned");
        Self::gc_locked(&mut guard);
    }

    fn gc_locked(map: &mut HashMap<String, ScrollContext>) {
        let now = Instant::now();
        map.retain(|_, ctx| ctx.expires_at > now);
    }
}

/// Parse a matchID-style keepalive string into a `Duration`.
///
/// Accepted suffixes: `ms`, `s`, `m`, `h`. Anything else falls back to
/// the default 1-minute TTL — matches ES behaviour, which never 400s
/// on an unparseable `scroll` keepalive.
pub fn parse_scroll_ttl(keepalive: &str) -> Duration {
    let trimmed = keepalive.trim();
    if trimmed.is_empty() {
        return DEFAULT_SCROLL_TTL;
    }
    // Split into digits + suffix (longest suffix first to handle `ms`).
    for (suffix, unit) in [
        ("ms", Duration::from_millis(1)),
        ("s", Duration::from_secs(1)),
        ("m", Duration::from_secs(60)),
        ("h", Duration::from_secs(3_600)),
    ] {
        if let Some(prefix) = trimmed.strip_suffix(suffix) {
            if let Ok(n) = prefix.trim().parse::<u64>() {
                return unit.saturating_mul(n.min(u32::MAX as u64) as u32);
            }
        }
    }
    // Bare integer → seconds, matches ES.
    if let Ok(n) = trimmed.parse::<u64>() {
        return Duration::from_secs(n);
    }
    DEFAULT_SCROLL_TTL
}

fn generate_scroll_id() -> String {
    // UUID v4 → 32 hex chars, opaque + URL-safe + collision-resistant.
    // Matches the "base64-ish random" shape we promised; clients
    // treat the id as opaque so the exact encoding does not matter.
    Uuid::new_v4().simple().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::SearchRequest;

    fn empty_request() -> SearchRequest {
        SearchRequest {
            query: None,
            from: None,
            size: None,
            source: None,
            track_total_hits: None,
            sort: Vec::new(),
            highlight: None,
            min_score: None,
            aggs: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn parse_scroll_ttl_known_suffixes() {
        assert_eq!(parse_scroll_ttl("1m"), Duration::from_secs(60));
        assert_eq!(parse_scroll_ttl("30s"), Duration::from_secs(30));
        assert_eq!(parse_scroll_ttl("2h"), Duration::from_secs(7_200));
        assert_eq!(parse_scroll_ttl("500ms"), Duration::from_millis(500));
        assert_eq!(parse_scroll_ttl("5"), Duration::from_secs(5));
    }

    #[test]
    fn parse_scroll_ttl_garbage_falls_back_to_default() {
        assert_eq!(parse_scroll_ttl(""), DEFAULT_SCROLL_TTL);
        assert_eq!(parse_scroll_ttl("nope"), DEFAULT_SCROLL_TTL);
        assert_eq!(parse_scroll_ttl("1d"), DEFAULT_SCROLL_TTL);
    }

    #[test]
    fn scroll_table_insert_take_roundtrip() {
        let table = ScrollTable::new();
        let ctx = ScrollContext {
            index: "deces".into(),
            request: empty_request(),
            cursor: 0,
            size: 1000,
            expires_at: Instant::now() + Duration::from_secs(60),
        };
        let id = table.insert(ctx);
        let taken = table.take(&id).expect("scroll id should be present");
        assert_eq!(taken.index, "deces");
        assert_eq!(taken.cursor, 0);
        assert_eq!(taken.size, 1000);
        assert!(table.take(&id).is_none(), "second take should miss");
    }

    #[test]
    fn scroll_table_unknown_id_returns_none() {
        let table = ScrollTable::new();
        assert!(table.take("does-not-exist").is_none());
    }

    #[test]
    fn scroll_table_drops_expired_entries() {
        let table = ScrollTable::new();
        let expired = ScrollContext {
            index: "deces".into(),
            request: empty_request(),
            cursor: 0,
            size: 1000,
            expires_at: Instant::now() - Duration::from_secs(1),
        };
        let id = table.insert(expired);
        // The next op runs gc_expired and evicts the stale entry.
        assert!(table.take(&id).is_none());
    }
}
