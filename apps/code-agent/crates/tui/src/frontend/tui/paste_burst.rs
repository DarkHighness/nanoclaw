use std::time::{Duration, Instant};

#[cfg(not(windows))]
const PASTE_BURST_CHAR_INTERVAL: Duration = Duration::from_millis(8);
#[cfg(windows)]
const PASTE_BURST_CHAR_INTERVAL: Duration = Duration::from_millis(30);

#[cfg(not(windows))]
const PASTE_BURST_ACTIVE_IDLE_TIMEOUT: Duration = Duration::from_millis(8);
#[cfg(windows)]
const PASTE_BURST_ACTIVE_IDLE_TIMEOUT: Duration = Duration::from_millis(60);

const PASTE_BURST_MIN_CHARS: u16 = 2;
const PASTE_ENTER_SUPPRESS_WINDOW: Duration = Duration::from_millis(120);

#[derive(Default)]
pub(crate) struct PasteBurst {
    last_plain_char_time: Option<Instant>,
    consecutive_plain_char_burst: u16,
    burst_window_until: Option<Instant>,
    buffer: String,
    active: bool,
    pending_first_char: Option<(char, Instant)>,
}

pub(crate) enum CharDecision {
    BufferAppend,
    RetainFirstChar,
    BeginBufferFromPending,
}

pub(crate) enum FlushResult {
    Paste(String),
    Typed(char),
    None,
}

impl PasteBurst {
    #[cfg(test)]
    pub(crate) fn recommended_flush_delay() -> Duration {
        PASTE_BURST_CHAR_INTERVAL + Duration::from_millis(1)
    }

    #[cfg(test)]
    pub(crate) fn recommended_active_flush_delay() -> Duration {
        PASTE_BURST_ACTIVE_IDLE_TIMEOUT + Duration::from_millis(1)
    }

    pub(crate) fn on_plain_char(&mut self, ch: char, now: Instant) -> CharDecision {
        self.note_plain_char(now);

        if self.active {
            self.burst_window_until = Some(now + PASTE_ENTER_SUPPRESS_WINDOW);
            return CharDecision::BufferAppend;
        }

        if let Some((held, held_at)) = self.pending_first_char
            && now.duration_since(held_at) <= PASTE_BURST_CHAR_INTERVAL
            && self.consecutive_plain_char_burst >= PASTE_BURST_MIN_CHARS
        {
            self.active = true;
            let _ = self.pending_first_char.take();
            self.buffer.push(held);
            self.burst_window_until = Some(now + PASTE_ENTER_SUPPRESS_WINDOW);
            return CharDecision::BeginBufferFromPending;
        }

        self.pending_first_char = Some((ch, now));
        CharDecision::RetainFirstChar
    }

    pub(crate) fn on_plain_char_no_hold(&mut self, now: Instant) -> bool {
        self.note_plain_char(now);
        if self.active {
            self.burst_window_until = Some(now + PASTE_ENTER_SUPPRESS_WINDOW);
            return true;
        }
        false
    }

    pub(crate) fn append_char_to_buffer(&mut self, ch: char, now: Instant) {
        self.buffer.push(ch);
        self.active = true;
        self.burst_window_until = Some(now + PASTE_ENTER_SUPPRESS_WINDOW);
    }

    pub(crate) fn append_newline_if_active(&mut self, now: Instant) -> bool {
        if !self.is_active_internal() {
            return false;
        }
        self.buffer.push('\n');
        self.active = true;
        self.burst_window_until = Some(now + PASTE_ENTER_SUPPRESS_WINDOW);
        true
    }

    pub(crate) fn newline_should_insert_instead_of_submit(&self, now: Instant) -> bool {
        self.burst_window_until
            .is_some_and(|until| now <= until && !self.is_active_internal())
    }

    pub(crate) fn flush_if_due(&mut self, now: Instant) -> FlushResult {
        let timeout = if self.is_active_internal() {
            PASTE_BURST_ACTIVE_IDLE_TIMEOUT
        } else {
            PASTE_BURST_CHAR_INTERVAL
        };
        let timed_out = self
            .last_plain_char_time
            .is_some_and(|last| now.duration_since(last) > timeout);
        if !timed_out {
            return FlushResult::None;
        }

        if self.is_active_internal() {
            self.active = false;
            return FlushResult::Paste(std::mem::take(&mut self.buffer));
        }

        if let Some((ch, _)) = self.pending_first_char.take() {
            return FlushResult::Typed(ch);
        }

        FlushResult::None
    }

    pub(crate) fn flush_before_modified_input(&mut self) -> Option<String> {
        if !self.is_active() {
            return None;
        }

        let mut out = std::mem::take(&mut self.buffer);
        if let Some((ch, _)) = self.pending_first_char.take() {
            out.push(ch);
        }
        self.active = false;
        Some(out)
    }

    pub(crate) fn clear_window_after_non_char(&mut self) {
        self.consecutive_plain_char_burst = 0;
        self.last_plain_char_time = None;
        self.burst_window_until = None;
        self.active = false;
        self.pending_first_char = None;
    }

    pub(crate) fn clear_after_explicit_paste(&mut self) {
        self.clear_window_after_non_char();
        self.buffer.clear();
    }

    fn note_plain_char(&mut self, now: Instant) {
        match self.last_plain_char_time {
            Some(previous) if now.duration_since(previous) <= PASTE_BURST_CHAR_INTERVAL => {
                self.consecutive_plain_char_burst =
                    self.consecutive_plain_char_burst.saturating_add(1);
            }
            _ => self.consecutive_plain_char_burst = 1,
        }
        self.last_plain_char_time = Some(now);
    }

    fn is_active(&self) -> bool {
        self.is_active_internal() || self.pending_first_char.is_some()
    }

    fn is_active_internal(&self) -> bool {
        self.active || !self.buffer.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{CharDecision, FlushResult, PasteBurst};
    use std::time::{Duration, Instant};

    #[test]
    fn ascii_first_char_flushes_as_typed_when_no_burst_follows() {
        let mut burst = PasteBurst::default();
        let t0 = Instant::now();

        assert!(matches!(
            burst.on_plain_char('a', t0),
            CharDecision::RetainFirstChar
        ));

        let t1 = t0 + PasteBurst::recommended_flush_delay() + Duration::from_millis(1);
        assert!(matches!(burst.flush_if_due(t1), FlushResult::Typed('a')));
    }

    #[test]
    fn fast_ascii_chars_flush_as_one_paste() {
        let mut burst = PasteBurst::default();
        let t0 = Instant::now();

        assert!(matches!(
            burst.on_plain_char('a', t0),
            CharDecision::RetainFirstChar
        ));

        let t1 = t0 + Duration::from_millis(1);
        assert!(matches!(
            burst.on_plain_char('b', t1),
            CharDecision::BeginBufferFromPending
        ));
        burst.append_char_to_buffer('b', t1);

        let t2 = t1 + Duration::from_millis(1);
        assert!(matches!(
            burst.on_plain_char('c', t2),
            CharDecision::BufferAppend
        ));
        burst.append_char_to_buffer('c', t2);

        let t3 = t2 + PasteBurst::recommended_active_flush_delay() + Duration::from_millis(1);
        assert!(matches!(
            burst.flush_if_due(t3),
            FlushResult::Paste(ref text) if text == "abc"
        ));
    }

    #[test]
    fn enter_extends_the_current_burst_with_a_newline() {
        let mut burst = PasteBurst::default();
        let t0 = Instant::now();

        assert!(matches!(
            burst.on_plain_char('a', t0),
            CharDecision::RetainFirstChar
        ));

        let t1 = t0 + Duration::from_millis(1);
        assert!(matches!(
            burst.on_plain_char('b', t1),
            CharDecision::BeginBufferFromPending
        ));
        burst.append_char_to_buffer('b', t1);
        assert!(burst.append_newline_if_active(t1 + Duration::from_millis(1)));
    }

    #[test]
    fn modified_input_flushes_held_characters_immediately() {
        let mut burst = PasteBurst::default();
        let t0 = Instant::now();

        assert!(matches!(
            burst.on_plain_char('a', t0),
            CharDecision::RetainFirstChar
        ));
        assert_eq!(burst.flush_before_modified_input(), Some("a".to_string()));
    }
}
