//! The daemon's log tail: a `tracing` layer that writes sanitized, redacted,
//! bounded lines into an in-memory ring buffer, with a broadcast channel for
//! live streams.
//!
//! Concurrency rules: the buffer's `std::sync::Mutex` is taken only inside
//! [`LogBuffer::push`] and the read paths, never across an `.await`, and
//! nothing done while holding it can emit a `tracing` event (this module and
//! `logs/redact.rs` contain no logging macros; a test checks it).

mod redact;

use std::collections::VecDeque;
use std::fmt::{self, Write as _};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex, MutexGuard, OnceLock, PoisonError};

use serde::Serialize;
use tokio::sync::broadcast;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

use self::redact::is_secret_field;
pub(crate) use self::redact::redact;

pub(crate) const LOG_BUFFER_LINES: usize = 2_000;
pub(crate) const LOG_LINE_MAX_BYTES: usize = 4_096;
pub(crate) const LOG_REDACTED: &str = "[redacted]";
pub(crate) const LOG_TRUNCATED_SUFFIX: &str = "…[truncated]";
pub(crate) const LOG_BROADCAST_CAPACITY: usize = 256;
/// How much of a message is read before redaction.
const LOG_INPUT_MAX_BYTES: usize = 65_536;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        match text.to_ascii_lowercase().as_str() {
            "trace" => Some(Self::Trace),
            "debug" => Some(Self::Debug),
            "info" => Some(Self::Info),
            "warn" => Some(Self::Warn),
            "error" => Some(Self::Error),
            _ => None,
        }
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

impl From<tracing::Level> for LogLevel {
    fn from(level: tracing::Level) -> Self {
        if level == tracing::Level::ERROR {
            Self::Error
        } else if level == tracing::Level::WARN {
            Self::Warn
        } else if level == tracing::Level::INFO {
            Self::Info
        } else if level == tracing::Level::DEBUG {
            Self::Debug
        } else {
            Self::Trace
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LogLine {
    pub(crate) seq: u64,
    pub(crate) at: u64,
    pub(crate) level: LogLevel,
    pub(crate) target: String,
    pub(crate) message: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct LogFilter {
    pub(crate) min_level: Option<LogLevel>,
    /// Lowercased by [`LogFilter::new`].
    pub(crate) query: Option<String>,
}

impl LogFilter {
    pub(crate) fn new(min_level: Option<LogLevel>, query: Option<&str>) -> Self {
        Self {
            min_level,
            query: query
                .filter(|query| !query.is_empty())
                .map(str::to_lowercase),
        }
    }

    /// The line's level is at least `min_level`, and `query` is a
    /// case-insensitive substring of its message or target.
    pub(crate) fn matches(&self, line: &LogLine) -> bool {
        if self.min_level.is_some_and(|min| line.level < min) {
            return false;
        }
        match &self.query {
            None => true,
            Some(query) => {
                line.message.to_lowercase().contains(query.as_str())
                    || line.target.to_lowercase().contains(query.as_str())
            }
        }
    }
}

struct Inner {
    lines: VecDeque<Arc<LogLine>>,
    capacity: usize,
    newest_seq: u64,
}

pub(crate) struct LogBuffer {
    inner: StdMutex<Inner>,
    sender: broadcast::Sender<Arc<LogLine>>,
    streams: AtomicUsize,
}

impl LogBuffer {
    pub(crate) fn new() -> Arc<Self> {
        Self::with_limits(LOG_BUFFER_LINES, LOG_BROADCAST_CAPACITY)
    }

    pub(crate) fn with_limits(lines: usize, channel: usize) -> Arc<Self> {
        let capacity = lines.max(1);
        let (sender, _) = broadcast::channel(channel.max(1));
        Arc::new(Self {
            inner: StdMutex::new(Inner {
                lines: VecDeque::with_capacity(capacity.min(LOG_BUFFER_LINES)),
                capacity,
                newest_seq: 0,
            }),
            sender,
            streams: AtomicUsize::new(0),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Sanitizes (control characters and ANSI removed), redacts, truncates,
    /// assigns the next seq, stores, and broadcasts. Returns the seq.
    pub(crate) fn push(&self, at_ms: u64, level: LogLevel, target: &str, message: &str) -> u64 {
        // All text work happens before the lock is taken.
        let target = sanitize(cap_input(target).0);
        let message = prepare_message(message);
        let mut inner = self.lock();
        inner.newest_seq += 1;
        let seq = inner.newest_seq;
        let line = Arc::new(LogLine {
            seq,
            at: at_ms,
            level,
            target,
            message,
        });
        inner.lines.push_back(Arc::clone(&line));
        while inner.lines.len() > inner.capacity {
            inner.lines.pop_front();
        }
        // No receiver is not an error here, and `send` never logs.
        let _ = self.sender.send(line);
        seq
    }

    /// Oldest first. No `after`: the newest `limit` matches. With `after`:
    /// the first `limit` matches with seq > after.
    pub(crate) fn lines(
        &self,
        filter: &LogFilter,
        after: Option<u64>,
        limit: usize,
    ) -> Vec<LogLine> {
        let selected: Vec<Arc<LogLine>> = {
            let inner = self.lock();
            match after {
                None => {
                    let mut newest: Vec<Arc<LogLine>> = inner
                        .lines
                        .iter()
                        .rev()
                        .filter(|line| filter.matches(line))
                        .take(limit)
                        .cloned()
                        .collect();
                    newest.reverse();
                    newest
                }
                Some(after) => inner
                    .lines
                    .iter()
                    .filter(|line| line.seq > after && filter.matches(line))
                    .take(limit)
                    .cloned()
                    .collect(),
            }
        };
        selected.iter().map(|line| LogLine::clone(line)).collect()
    }

    /// The newest seq assigned; 0 before the first line.
    pub(crate) fn newest_seq(&self) -> u64 {
        self.lock().newest_seq
    }

    /// Lines held now, for status and metrics.
    pub(crate) fn buffered(&self) -> usize {
        self.lock().lines.len()
    }

    /// The lines with seq > after and a receiver, taken under one lock hold,
    /// so no line falls between them.
    pub(crate) fn subscribe_after(
        &self,
        after: u64,
    ) -> (Vec<Arc<LogLine>>, broadcast::Receiver<Arc<LogLine>>) {
        let inner = self.lock();
        let receiver = self.sender.subscribe();
        let backlog = inner
            .lines
            .iter()
            .filter(|line| line.seq > after)
            .cloned()
            .collect();
        (backlog, receiver)
    }

    /// Takes one of `max` stream slots, or `None` when all are taken. The
    /// guard's `Drop` frees the slot.
    pub(crate) fn try_open_stream(self: &Arc<Self>, max: usize) -> Option<StreamGuard> {
        let mut current = self.streams.load(Ordering::Acquire);
        loop {
            if current >= max {
                return None;
            }
            match self.streams.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return Some(StreamGuard {
                        buffer: Arc::clone(self),
                    })
                }
                Err(actual) => current = actual,
            }
        }
    }
}

/// One open log stream's slot; dropping it frees the slot.
pub(crate) struct StreamGuard {
    buffer: Arc<LogBuffer>,
}

impl Drop for StreamGuard {
    fn drop(&mut self) {
        self.buffer.streams.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The process-wide buffer `init_tracing` captures into.
pub(crate) fn global() -> Arc<LogBuffer> {
    static GLOBAL: OnceLock<Arc<LogBuffer>> = OnceLock::new();
    Arc::clone(GLOBAL.get_or_init(LogBuffer::new))
}

/// Caps, sanitizes, redacts, then cuts to [`LOG_LINE_MAX_BYTES`]. Redaction
/// sees the whole capped text before the cut, so a secret straddling the cut
/// is replaced whole and never half-shown.
fn prepare_message(message: &str) -> String {
    let (capped, was_capped) = cap_input(message);
    let redacted = redact(&sanitize(capped));
    if !was_capped && redacted.len() <= LOG_LINE_MAX_BYTES {
        return redacted;
    }
    let mut line = redacted;
    line.truncate(floor_char_boundary(
        &line,
        LOG_LINE_MAX_BYTES - LOG_TRUNCATED_SUFFIX.len(),
    ));
    line.push_str(LOG_TRUNCATED_SUFFIX);
    line
}

/// Caps the input at [`LOG_INPUT_MAX_BYTES`] on a char boundary. The cap can
/// split a secret, and a split secret may no longer match a pattern, so the
/// partial last word is dropped too.
fn cap_input(text: &str) -> (&str, bool) {
    if text.len() <= LOG_INPUT_MAX_BYTES {
        return (text, false);
    }
    let capped = &text[..floor_char_boundary(text, LOG_INPUT_MAX_BYTES)];
    let capped = match capped.rfind(char::is_whitespace) {
        Some(index) => &capped[..index],
        None => capped,
    };
    (capped, true)
}

fn floor_char_boundary(text: &str, index: usize) -> usize {
    if index >= text.len() {
        return text.len();
    }
    let mut index = index;
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// Removes ANSI escape sequences whole and replaces every other control
/// character (newline and tab included) with a single space.
fn sanitize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.peek() {
                // CSI: parameter and intermediate bytes, then one final byte.
                Some('[') => {
                    chars.next();
                    while let Some(&next) = chars.peek() {
                        if ('\u{20}'..='\u{3f}').contains(&next) {
                            chars.next();
                            continue;
                        }
                        if ('\u{40}'..='\u{7e}').contains(&next) {
                            chars.next();
                        }
                        break;
                    }
                }
                // OSC: up to BEL or the string terminator.
                Some(']') => {
                    chars.next();
                    while let Some(&next) = chars.peek() {
                        if next == '\u{7}' {
                            chars.next();
                            break;
                        }
                        if next == '\u{1b}' {
                            chars.next();
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                        if next.is_control() {
                            break;
                        }
                        chars.next();
                    }
                }
                Some(next) if !next.is_control() => {
                    chars.next();
                }
                _ => {}
            }
            continue;
        }
        out.push(if ch.is_control() { ' ' } else { ch });
    }
    out
}

/// Collects an event's text: the `message` field, then every other field as
/// ` name=value`. A field whose name looks secret is written as
/// `name=[redacted]` without its value being formatted.
#[derive(Default)]
struct LineVisitor {
    message: String,
    fields: String,
}

impl LineVisitor {
    fn finish(self) -> String {
        if self.message.is_empty() {
            self.fields.trim_start().to_string()
        } else {
            self.message + &self.fields
        }
    }

    fn push_redacted(&mut self, name: &str) {
        let _ = write!(self.fields, " {name}={LOG_REDACTED}");
    }
}

impl Visit for LineVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        let name = field.name();
        if name == "message" {
            self.message.push_str(value);
        } else if is_secret_field(name) {
            self.push_redacted(name);
        } else if value.is_empty() || value.chars().any(char::is_whitespace) {
            let _ = write!(self.fields, " {name}={value:?}");
        } else {
            let _ = write!(self.fields, " {name}={value}");
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        let name = field.name();
        if name == "message" {
            let _ = write!(self.message, "{value:?}");
        } else if is_secret_field(name) {
            self.push_redacted(name);
        } else {
            let _ = write!(self.fields, " {name}={value:?}");
        }
    }
}

/// Writes every event that reaches it into a [`LogBuffer`].
pub(crate) struct LogLayer {
    buffer: Arc<LogBuffer>,
}

impl LogLayer {
    pub(crate) fn new(buffer: Arc<LogBuffer>) -> Self {
        Self { buffer }
    }
}

impl<S: Subscriber> Layer<S> for LogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let mut visitor = LineVisitor::default();
        event.record(&mut visitor);
        // Everything above runs without the buffer lock; `push` takes it once,
        // briefly, and neither it nor anything it calls may log.
        self.buffer.push(
            anima_core::primitives::now_millis(),
            LogLevel::from(*meta.level()),
            meta.target(),
            &visitor.finish(),
        );
    }
}

/// Env filter (default `anima_daemon=info,tower_http=info`), the compact fmt
/// layer, and the log layer on [`global`]. A second call is a no-op.
pub fn init_tracing() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new("anima_daemon=info,tower_http=info")
    });
    let fmt_layer = tracing_subscriber::fmt::layer()
        .with_target(false)
        .compact();
    // Under the test harness the fmt output goes through its capture.
    #[cfg(test)]
    let fmt_layer = fmt_layer.with_test_writer();
    let _ = tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt_layer)
        .with(LogLayer::new(global()))
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    use anima_core::primitives::now_millis;
    use tokio::sync::broadcast::error::TryRecvError;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::Registry;

    fn capture(buffer: &Arc<LogBuffer>, emit: impl FnOnce()) {
        let subscriber = Registry::default().with(LogLayer::new(Arc::clone(buffer)));
        tracing::subscriber::with_default(subscriber, emit);
    }

    fn all_lines(buffer: &LogBuffer) -> Vec<LogLine> {
        buffer.lines(&LogFilter::default(), None, usize::MAX)
    }

    fn only_line(buffer: &LogBuffer) -> LogLine {
        let lines = all_lines(buffer);
        assert_eq!(lines.len(), 1, "{lines:?}");
        lines.into_iter().next().unwrap()
    }

    #[test]
    fn captures_message_target_level_and_time() {
        let buffer = LogBuffer::new();
        let before = now_millis();
        capture(&buffer, || {
            tracing::warn!(target: "anima_daemon::probe", "hello {}", "world");
        });
        let after = now_millis();

        let line = only_line(&buffer);
        assert_eq!(line.seq, 1);
        assert_eq!(line.level, LogLevel::Warn);
        assert_eq!(line.target, "anima_daemon::probe");
        assert_eq!(line.message, "hello world");
        assert!(before <= line.at && line.at <= after, "{line:?}");
    }

    #[test]
    fn fields_are_appended_as_key_value() {
        let buffer = LogBuffer::new();
        capture(&buffer, || {
            tracing::info!(
                agent_id = "a1",
                count = 3,
                note = "two words",
                empty = "",
                ok = true,
                "started"
            );
            tracing::info!(agent_id = "a2");
        });

        let lines = all_lines(&buffer);
        assert_eq!(
            lines[0].message,
            r#"started agent_id=a1 count=3 note="two words" empty="" ok=true"#
        );
        assert_eq!(lines[1].message, "agent_id=a2");
    }

    struct Explodes;

    impl fmt::Debug for Explodes {
        fn fmt(&self, _: &mut fmt::Formatter<'_>) -> fmt::Result {
            panic!("a secret field's value was formatted")
        }
    }

    #[test]
    fn a_secret_field_name_redacts_its_value_without_reading_it() {
        let buffer = LogBuffer::new();
        capture(&buffer, || {
            tracing::info!(
                api_key = ?Explodes,
                token = "abc",
                authorization = %"Bearer abc",
                prompt_tokens = 12,
                "calling"
            );
        });

        assert_eq!(
            only_line(&buffer).message,
            "calling api_key=[redacted] token=[redacted] authorization=[redacted] prompt_tokens=12"
        );
    }

    #[test]
    fn a_secret_in_the_message_is_redacted() {
        let buffer = LogBuffer::new();
        capture(&buffer, || {
            tracing::warn!(
                "call failed: Authorization: Bearer abc.def-ghi key=sk-ant-api03-AbCdEfGhIjKl"
            );
        });

        let message = only_line(&buffer).message;
        assert!(!message.contains("abc.def-ghi"), "{message}");
        assert!(!message.contains("sk-ant-api03"), "{message}");
        assert!(message.contains(LOG_REDACTED), "{message}");
    }

    #[test]
    fn control_characters_and_ansi_are_removed() {
        let buffer = LogBuffer::new();
        buffer.push(
            0,
            LogLevel::Info,
            "anima\ndaemon",
            "a\u{1b}[31mred\u{1b}[0m\nnext\tline\r\u{7}end\u{1b}]0;title\u{7}!",
        );

        let line = only_line(&buffer);
        assert_eq!(line.message, "ared next line  end!");
        assert_eq!(line.target, "anima daemon");
        assert!(!line.message.chars().any(char::is_control));
    }

    #[test]
    fn capture_is_bounded() {
        let buffer = LogBuffer::new();
        for index in 0..2_500 {
            buffer.push(index, LogLevel::Info, "t", &format!("line {index}"));
        }

        let lines = all_lines(&buffer);
        assert_eq!(buffer.buffered(), LOG_BUFFER_LINES);
        assert_eq!(lines.len(), LOG_BUFFER_LINES);
        assert_eq!(lines[0].seq, 501);
        assert_eq!(buffer.newest_seq(), 2_500);
        for pair in lines.windows(2) {
            assert_eq!(pair[1].seq, pair[0].seq + 1);
        }
    }

    #[test]
    fn a_long_line_is_cut_on_a_char_boundary_with_the_suffix() {
        for (prefix, unit) in [("", "é"), ("a", "é"), ("ab", "€"), ("abc", "😀")] {
            let buffer = LogBuffer::new();
            let message = format!("{prefix}{}", unit.repeat(5_000));
            buffer.push(0, LogLevel::Info, "t", &message);

            let line = only_line(&buffer).message;
            assert!(line.len() <= LOG_LINE_MAX_BYTES, "{}", line.len());
            assert!(line.len() > LOG_LINE_MAX_BYTES - 4 - LOG_TRUNCATED_SUFFIX.len());
            assert!(line.ends_with(LOG_TRUNCATED_SUFFIX));
            assert!(line.starts_with(prefix));
        }

        let buffer = LogBuffer::new();
        let exact = "a".repeat(LOG_LINE_MAX_BYTES);
        buffer.push(0, LogLevel::Info, "t", &exact);
        assert_eq!(only_line(&buffer).message, exact);
    }

    #[test]
    fn redaction_survives_truncation_boundaries() {
        let secrets = [
            "Q7wErTy9uIoP3aSdF6gHjK2lZxCvB8nM4qWeRt5Y",
            "sk-ant-api03-ZyXwVuTsRqPoNmLkJiHgFe",
        ];
        for secret in secrets {
            for start in (LOG_LINE_MAX_BYTES - secret.len() - 20)..(LOG_LINE_MAX_BYTES + 4) {
                let buffer = LogBuffer::new();
                let message = format!("{} {secret} tail", ".".repeat(start - 1));
                buffer.push(0, LogLevel::Info, "t", &message);

                let line = only_line(&buffer).message;
                for window in secret.as_bytes().windows(4) {
                    let window = std::str::from_utf8(window).unwrap();
                    assert!(
                        !line.contains(window),
                        "{window:?} of {secret:?} at {start} survived"
                    );
                }
            }
        }
    }

    #[test]
    fn a_capped_input_drops_its_partial_last_word() {
        let buffer = LogBuffer::new();
        let message = format!(
            "{} Q7wErTy9uIoP3aSdF6gHjK2lZxCvB8nM4qWeRt5Y",
            "\u{1b}[0m".repeat(LOG_INPUT_MAX_BYTES / 4 - 4)
        );
        buffer.push(0, LogLevel::Info, "t", &message);

        let line = only_line(&buffer).message;
        assert!(!line.contains("Q7wE"), "{line}");
        assert!(line.ends_with(LOG_TRUNCATED_SUFFIX), "{line}");
    }

    #[test]
    fn seq_never_repeats_under_concurrent_pushes() {
        let buffer = LogBuffer::new();
        std::thread::scope(|scope| {
            for thread in 0..8 {
                let buffer = &buffer;
                scope.spawn(move || {
                    for index in 0..200 {
                        buffer.push(0, LogLevel::Info, "t", &format!("{thread}:{index}"));
                    }
                });
            }
        });

        let mut seqs: Vec<u64> = all_lines(&buffer).iter().map(|line| line.seq).collect();
        seqs.sort_unstable();
        seqs.dedup();
        assert_eq!(seqs, (1..=1_600).collect::<Vec<u64>>());
    }

    #[test]
    fn a_poisoned_lock_does_not_stop_logging() {
        let buffer = LogBuffer::new();
        buffer.push(0, LogLevel::Info, "t", "before");
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = buffer.inner.lock().unwrap();
            panic!("poisoning the log buffer on purpose");
        }));
        assert!(poisoned.is_err());
        assert!(buffer.inner.is_poisoned());

        assert_eq!(buffer.push(0, LogLevel::Info, "t", "after"), 2);
        assert_eq!(all_lines(&buffer)[1].message, "after");
    }

    #[test]
    fn level_filter_orders_error_above_warn_above_info() {
        assert!(LogLevel::Error > LogLevel::Warn);
        assert!(LogLevel::Warn > LogLevel::Info);
        assert!(LogLevel::Info > LogLevel::Debug);
        assert!(LogLevel::Debug > LogLevel::Trace);
        for level in [
            LogLevel::Trace,
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warn,
            LogLevel::Error,
        ] {
            assert_eq!(LogLevel::parse(level.as_str()), Some(level));
        }
        assert_eq!(LogLevel::parse("WARN"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::parse("warning"), None);

        let buffer = LogBuffer::new();
        for level in [
            LogLevel::Trace,
            LogLevel::Debug,
            LogLevel::Info,
            LogLevel::Warn,
            LogLevel::Error,
        ] {
            buffer.push(0, level, "t", level.as_str());
        }
        let warn_and_up = buffer.lines(&LogFilter::new(Some(LogLevel::Warn), None), None, 10);
        let levels: Vec<LogLevel> = warn_and_up.iter().map(|line| line.level).collect();
        assert_eq!(levels, [LogLevel::Warn, LogLevel::Error]);
    }

    #[test]
    fn query_filter_is_case_insensitive_over_message_and_target() {
        let buffer = LogBuffer::new();
        buffer.push(0, LogLevel::Info, "anima_daemon::history", "Flush Failed");
        buffer.push(0, LogLevel::Info, "tower_http::trace", "request done");

        let matching = |query: &str| {
            buffer
                .lines(&LogFilter::new(None, Some(query)), None, 10)
                .into_iter()
                .map(|line| line.seq)
                .collect::<Vec<u64>>()
        };
        assert_eq!(matching("FLUSH"), [1]);
        assert_eq!(matching("HISTORY"), [1]);
        assert_eq!(matching("Tower_HTTP"), [2]);
        assert_eq!(matching("nothing"), Vec::<u64>::new());
        assert_eq!(matching(""), [1, 2]);
    }

    #[test]
    fn lines_take_the_newest_or_the_first_after() {
        let buffer = LogBuffer::new();
        for index in 0..5 {
            buffer.push(0, LogLevel::Info, "t", &format!("{index}"));
        }
        let seqs = |lines: Vec<LogLine>| lines.iter().map(|line| line.seq).collect::<Vec<u64>>();

        assert_eq!(seqs(buffer.lines(&LogFilter::default(), None, 2)), [4, 5]);
        assert_eq!(
            seqs(buffer.lines(&LogFilter::default(), Some(1), 2)),
            [2, 3]
        );
        assert_eq!(
            seqs(buffer.lines(&LogFilter::default(), Some(5), 2)),
            Vec::<u64>::new()
        );
    }

    #[test]
    fn subscribe_after_returns_the_backlog_and_then_live_lines_without_a_gap() {
        let buffer = LogBuffer::new();
        for index in 1..=3 {
            buffer.push(0, LogLevel::Info, "t", &format!("{index}"));
        }

        let (backlog, mut receiver) = buffer.subscribe_after(1);
        let backlog: Vec<u64> = backlog.iter().map(|line| line.seq).collect();
        assert_eq!(backlog, [2, 3]);

        buffer.push(0, LogLevel::Info, "t", "4");
        assert_eq!(receiver.try_recv().unwrap().seq, 4);
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn a_stream_slot_is_freed_when_its_guard_drops() {
        let buffer = LogBuffer::new();
        let first = buffer.try_open_stream(2).expect("first slot");
        let _second = buffer.try_open_stream(2).expect("second slot");
        assert!(buffer.try_open_stream(2).is_none());

        drop(first);
        assert!(buffer.try_open_stream(2).is_some());
    }

    struct CountingLayer(Arc<AtomicUsize>);

    impl<S: Subscriber> Layer<S> for CountingLayer {
        fn on_event(&self, _event: &Event<'_>, _ctx: Context<'_, S>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn the_layer_emits_no_events_of_its_own() {
        let buffer = LogBuffer::new();
        let counted = Arc::new(AtomicUsize::new(0));
        let subscriber = Registry::default()
            .with(LogLayer::new(Arc::clone(&buffer)))
            .with(CountingLayer(Arc::clone(&counted)));
        tracing::subscriber::with_default(subscriber, || {
            for index in 0..5 {
                tracing::info!(index, "event {index}");
            }
            tracing::error!(api_key = "sk-ant-api03-AbCdEfGhIjKl", "token=abc");
        });

        assert_eq!(counted.load(Ordering::SeqCst), 6);
        assert_eq!(buffer.buffered(), 6);
    }

    #[test]
    fn the_logging_modules_contain_no_tracing_macros() {
        for source in [include_str!("logs.rs"), include_str!("logs/redact.rs")] {
            let source = source.replace("\r\n", "\n");
            let production = source
                .split("\n#[cfg(test)]\nmod tests")
                .next()
                .unwrap_or_default();
            assert!(production.len() < source.len(), "the test module was found");
            for needle in [
                "info!(", "warn!(", "error!(", "debug!(", "trace!(", "event!(",
            ] {
                assert!(!production.contains(needle), "{needle}");
            }
        }
    }

    #[test]
    fn init_tracing_twice_does_not_panic() {
        init_tracing();
        init_tracing();
        assert!(tracing::dispatcher::has_been_set());
    }

    #[test]
    fn log_lines_serialize_in_camel_case_with_lowercase_levels() {
        let line = LogLine {
            seq: 1,
            at: 2,
            level: LogLevel::Warn,
            target: "t".into(),
            message: "m".into(),
        };
        assert_eq!(
            serde_json::to_value(&line).unwrap(),
            serde_json::json!({ "seq": 1, "at": 2, "level": "warn", "target": "t", "message": "m" })
        );
    }

    #[test]
    fn constants() {
        assert_eq!(LOG_BUFFER_LINES, 2_000);
        assert_eq!(LOG_LINE_MAX_BYTES, 4_096);
        assert_eq!(LOG_REDACTED, "[redacted]");
        assert_eq!(LOG_TRUNCATED_SUFFIX, "…[truncated]");
        assert_eq!(LOG_BROADCAST_CAPACITY, 256);
        assert_eq!(LOG_INPUT_MAX_BYTES, 65_536);
    }
}
