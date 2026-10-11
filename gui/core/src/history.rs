//! Query history: the most recent statements run in this app session,
//! newest first. Kept in memory (statements may contain sensitive literals,
//! so nothing is written to disk) and capped at [`QueryHistory::CAPACITY`].

use chrono::{DateTime, Local};
use std::collections::VecDeque;
use std::time::Duration;

/// How a history entry ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryOutcome {
    Ok,
    /// The engine's (or the guard's) message.
    Error(String),
    Cancelled,
}

impl HistoryOutcome {
    pub fn is_ok(&self) -> bool {
        matches!(self, HistoryOutcome::Ok)
    }
}

/// One executed statement.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    pub sql: String,
    /// Label of the connection it ran on (session name or `database (host)`).
    pub connection_label: String,
    pub started_at: DateTime<Local>,
    pub elapsed: Duration,
    /// Rows returned (`None` for failures and plans).
    pub rows: Option<usize>,
    pub outcome: HistoryOutcome,
}

impl HistoryEntry {
    pub fn new(
        sql: impl Into<String>,
        connection_label: impl Into<String>,
        started_at: DateTime<Local>,
        elapsed: Duration,
        rows: Option<usize>,
        outcome: HistoryOutcome,
    ) -> Self {
        Self {
            sql: sql.into(),
            connection_label: connection_label.into(),
            started_at,
            elapsed,
            rows,
            outcome,
        }
    }

    /// An entry for a statement that just finished after `elapsed`.
    pub fn finished(
        sql: impl Into<String>,
        connection_label: impl Into<String>,
        elapsed: Duration,
        rows: Option<usize>,
        outcome: HistoryOutcome,
    ) -> Self {
        let started_at = chrono::Duration::from_std(elapsed)
            .map(|elapsed| Local::now() - elapsed)
            .unwrap_or_else(|_| Local::now());
        Self::new(sql, connection_label, started_at, elapsed, rows, outcome)
    }

    /// The statement on one line for list rows: whitespace runs collapse
    /// to a single space and the text is cut to `max_chars` with an ellipsis.
    pub fn preview(&self, max_chars: usize) -> String {
        preview_sql(&self.sql, max_chars)
    }

    /// `just now`, `5 min ago`, `3 h ago`, `yesterday`, `Mar 4`.
    pub fn age_label(&self, now: DateTime<Local>) -> String {
        relative_time(self.started_at, now)
    }

    /// [`Self::age_label`] relative to the current time.
    pub fn age(&self) -> String {
        relative_time(self.started_at, Local::now())
    }
}

/// `sql` on one line: whitespace runs collapse to a single space and the
/// text is cut to `max_chars` with an ellipsis.
pub fn preview_sql(sql: &str, max_chars: usize) -> String {
    let collapsed = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max_chars {
        return collapsed;
    }
    let mut cut: String = collapsed
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect();
    cut.push('…');
    cut
}

/// Human relative time of `then` seen from `now`.
pub fn relative_time(then: DateTime<Local>, now: DateTime<Local>) -> String {
    let seconds = now.signed_duration_since(then).num_seconds();
    if seconds < 10 {
        return "just now".to_string();
    }
    if seconds < 60 {
        return format!("{seconds} s ago");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes} min ago");
    }
    let hours = minutes / 60;
    if hours < 24 && then.date_naive() == now.date_naive() {
        return format!("{hours} h ago");
    }
    let days = now
        .date_naive()
        .signed_duration_since(then.date_naive())
        .num_days();
    match days {
        0 => format!("{hours} h ago"),
        1 => "yesterday".to_string(),
        2..=6 => format!("{days} days ago"),
        _ => then.format("%b %-d").to_string(),
    }
}

/// `12 ms`, `1.24 s`, `1 m 05 s`.
pub fn format_duration(elapsed: Duration) -> String {
    let millis = elapsed.as_millis();
    if millis < 1_000 {
        format!("{millis} ms")
    } else if millis < 60_000 {
        format!("{:.2} s", elapsed.as_secs_f64())
    } else {
        let seconds = elapsed.as_secs();
        format!("{} m {:02} s", seconds / 60, seconds % 60)
    }
}

/// Recent statements, newest first.
#[derive(Debug, Clone)]
pub struct QueryHistory {
    entries: VecDeque<HistoryEntry>,
    capacity: usize,
}

impl Default for QueryHistory {
    fn default() -> Self {
        Self::with_capacity(Self::CAPACITY)
    }
}

impl QueryHistory {
    /// Entries kept before the oldest is dropped.
    pub const CAPACITY: usize = 500;

    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            entries: VecDeque::with_capacity(capacity.min(64)),
            capacity,
        }
    }

    /// Record `entry` as the newest one. Blank statements are ignored.
    pub fn push(&mut self, entry: HistoryEntry) {
        if entry.sql.trim().is_empty() {
            return;
        }
        self.entries.push_front(entry);
        self.entries.truncate(self.capacity);
    }

    /// Newest first.
    pub fn iter(&self) -> impl Iterator<Item = &HistoryEntry> {
        self.entries.iter()
    }

    /// Newest first, at most `limit` entries.
    pub fn recent(&self, limit: usize) -> Vec<HistoryEntry> {
        self.entries.iter().take(limit).cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rstest::rstest;

    fn entry(sql: &str) -> HistoryEntry {
        HistoryEntry::new(
            sql,
            "shop (localhost)",
            Local::now(),
            Duration::from_millis(12),
            Some(3),
            HistoryOutcome::Ok,
        )
    }

    #[test]
    fn newest_entry_comes_first() {
        let mut history = QueryHistory::new();
        history.push(entry("select 1"));
        history.push(entry("select 2"));
        let sql: Vec<_> = history.iter().map(|e| e.sql.as_str()).collect();
        assert_eq!(sql, vec!["select 2", "select 1"]);
        assert_eq!(history.recent(1).len(), 1);
        assert_eq!(history.recent(1)[0].sql, "select 2");
    }

    #[test]
    fn capacity_drops_the_oldest() {
        let mut history = QueryHistory::with_capacity(3);
        for n in 0..5 {
            history.push(entry(&format!("select {n}")));
        }
        assert_eq!(history.len(), 3);
        let sql: Vec<_> = history.iter().map(|e| e.sql.as_str()).collect();
        assert_eq!(sql, vec!["select 4", "select 3", "select 2"]);
    }

    #[test]
    fn default_capacity_is_five_hundred() {
        let mut history = QueryHistory::new();
        for n in 0..(QueryHistory::CAPACITY + 20) {
            history.push(entry(&format!("select {n}")));
        }
        assert_eq!(history.len(), 500);
        assert_eq!(
            history.iter().next().map(|e| e.sql.as_str()),
            Some("select 519")
        );
    }

    #[test]
    fn blank_statements_are_not_recorded() {
        let mut history = QueryHistory::new();
        history.push(entry("  \n "));
        assert!(history.is_empty());
        history.push(entry("select 1"));
        history.clear();
        assert!(history.is_empty());
    }

    #[test]
    fn preview_collapses_whitespace_and_truncates() {
        let e = entry("select  *\n  from   orders\nwhere id = 1");
        assert_eq!(e.preview(80), "select * from orders where id = 1");
        assert_eq!(e.preview(10), "select * …");
        assert_eq!(entry("é".repeat(5).as_str()).preview(3), "éé…");
    }

    fn at(hour: u32, minute: u32, second: u32, day: u32) -> DateTime<Local> {
        Local
            .with_ymd_and_hms(2026, 3, day, hour, minute, second)
            .single()
            .expect("unambiguous local time")
    }

    #[rstest]
    #[case(at(12, 0, 0, 10), at(12, 0, 3, 10), "just now")]
    #[case(at(12, 0, 0, 10), at(12, 0, 42, 10), "42 s ago")]
    #[case(at(12, 0, 0, 10), at(12, 5, 0, 10), "5 min ago")]
    #[case(at(9, 0, 0, 10), at(12, 30, 0, 10), "3 h ago")]
    #[case(at(22, 0, 0, 9), at(8, 0, 0, 10), "yesterday")]
    #[case(at(12, 0, 0, 7), at(12, 0, 0, 10), "3 days ago")]
    #[case(at(12, 0, 0, 1), at(12, 0, 0, 20), "Mar 1")]
    fn relative_times(
        #[case] then: DateTime<Local>,
        #[case] now: DateTime<Local>,
        #[case] expected: &str,
    ) {
        assert_eq!(relative_time(then, now), expected);
    }

    #[rstest]
    #[case(Duration::from_millis(12), "12 ms")]
    #[case(Duration::from_millis(1240), "1.24 s")]
    #[case(Duration::from_secs(65), "1 m 05 s")]
    fn duration_labels(#[case] elapsed: Duration, #[case] expected: &str) {
        assert_eq!(format_duration(elapsed), expected);
    }

    #[test]
    fn finished_entries_start_before_now() {
        let entry = HistoryEntry::finished(
            "select 1",
            "shop",
            Duration::from_secs(90),
            Some(1),
            HistoryOutcome::Ok,
        );
        let started = Local::now().signed_duration_since(entry.started_at);
        assert!(started.num_seconds() >= 89, "{started:?}");
        assert_eq!(entry.age(), "1 min ago");
    }

    #[test]
    fn outcome_helpers() {
        assert!(HistoryOutcome::Ok.is_ok());
        assert!(!HistoryOutcome::Cancelled.is_ok());
        assert!(!HistoryOutcome::Error("boom".into()).is_ok());
    }
}
