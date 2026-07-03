//! Shared date-range walking for history endpoints capped at 30 days per query.
//!
//! `GET /orders/historical` and the trade-history endpoints share the same
//! server-side range rules: one query answers at most 30 days, a missing
//! `start_date` defaults to 7 days before `end_date`, and a missing `end_date`
//! defaults to `start_date` + 7 days (not "now"). [`plan`] validates a caller's
//! range against those rules and [`walk`] fetches it whole: newest-first
//! windows the endpoint accepts, every pagination cursor followed, duplicates
//! from window overlap dropped, and the merged result sorted newest-first.

use std::cmp::Reverse;
use std::collections::HashSet;
use std::future::Future;
use std::hash::Hash;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::model::Page;
use crate::model::common::Timestamp;
use crate::transport::now_unix_millis;

/// The endpoint's 30-day cap on one query, less a millisecond so an inclusive
/// server-side bounds check cannot tip over.
const MAX_WINDOW_MS: i64 = 30 * 24 * 60 * 60 * 1000 - 1;

/// The widest walk accepted, in windows (~8.2 years).
const MAX_WINDOWS: i64 = 100;

/// A validated range walk: `start..=end` in epoch milliseconds plus the cap on
/// the total number of items collected.
pub struct RangePlan {
    pub start: i64,
    pub end: i64,
    pub cap: usize,
}

/// Validates a caller's range for `method` (named in error messages):
/// `start_date` is required, `cursor` must be unset (the walk manages its own
/// pagination), `end_date` defaults to now, and absurdly wide ranges are
/// refused — at one query per 30 days, ~8 years is a hundred requests, and a
/// start that far back is almost certainly a typo (for example an epoch value
/// pasted in the wrong unit).
pub fn plan(
    method: &str,
    start_date: Option<i64>,
    end_date: Option<i64>,
    has_cursor: bool,
    limit: Option<u32>,
) -> Result<RangePlan> {
    if has_cursor {
        return Err(Error::invalid_request(format!(
            "{method} manages pagination itself; `cursor` must be unset"
        )));
    }
    let Some(start) = start_date else {
        return Err(Error::invalid_request(format!(
            "{method} needs `start_date`"
        )));
    };
    let end = end_date.unwrap_or_else(now_unix_millis);
    if end < start {
        return Err(Error::invalid_request(
            "`end_date` must not precede `start_date`",
        ));
    }
    let span = end - start;
    if span / MAX_WINDOW_MS >= MAX_WINDOWS {
        return Err(Error::invalid_request(format!(
            "range spans {} days (~{} years) — check that `start_date` is what you \
             meant, or narrow the range",
            span / (24 * 60 * 60 * 1000),
            span / (365 * 24 * 60 * 60 * 1000),
        )));
    }
    let cap = limit.map_or(usize::MAX, |l| usize::try_from(l).unwrap_or(usize::MAX));
    Ok(RangePlan { start, end, cap })
}

/// Fetches a planned range whole. `fetch(window_start, window_end, cursor)`
/// runs one query; `dedup_key` identifies an item across the window overlap;
/// `event_time` is the sort key of the merged, newest-first result.
///
/// The walk fires requests in quick succession, so a mid-walk 429 waits out
/// the server-advised delay and retries — but only a bounded number of times
/// per walk, so a hard limit still surfaces as an error.
pub async fn walk<T, K, Fetch, Fut>(
    plan: &RangePlan,
    mut fetch: Fetch,
    dedup_key: impl Fn(&T) -> K,
    event_time: impl Fn(&T) -> Timestamp,
) -> Result<Page<T>>
where
    Fetch: FnMut(i64, i64, Option<String>) -> Fut,
    Fut: Future<Output = Result<Page<T>>>,
    K: Eq + Hash,
{
    let mut items: Vec<T> = Vec::new();
    // Adjacent windows share their boundary instant so no convention of
    // inclusive/exclusive bounds can drop an item there; the key set drops
    // the duplicates that overlap can produce instead.
    let mut seen: HashSet<K> = HashSet::new();
    let mut rate_limit_budget = 10_u32;
    let mut timestamp = None;
    let mut window_end = plan.end;
    'windows: loop {
        let window_start = window_end.saturating_sub(MAX_WINDOW_MS).max(plan.start);
        let mut cursor: Option<String> = None;
        loop {
            let page = loop {
                match fetch(window_start, window_end, cursor.clone()).await {
                    Ok(page) => break page,
                    Err(e) if e.is_rate_limited() && rate_limit_budget > 0 => {
                        rate_limit_budget -= 1;
                        let wait = e
                            .retry_after()
                            .unwrap_or(Duration::from_millis(500))
                            .min(Duration::from_secs(30));
                        tokio::time::sleep(wait).await;
                    }
                    Err(e) => return Err(e),
                }
            };
            // The newest window comes first; its server timestamp stands for
            // the merged page.
            timestamp.get_or_insert(page.timestamp);
            for item in page.items {
                if seen.insert(dedup_key(&item)) {
                    items.push(item);
                    if items.len() >= plan.cap {
                        break 'windows;
                    }
                }
            }
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        if window_start <= plan.start {
            break;
        }
        window_end = window_start;
    }
    // Windows arrive newest-first but each window's internal order is the
    // server's; make the merged ordering deterministic.
    items.sort_by_key(|item| Reverse(event_time(item)));
    Ok(Page {
        items,
        next_cursor: None,
        timestamp: timestamp.unwrap_or_else(|| Timestamp::from_unix_millis(now_unix_millis())),
    })
}
