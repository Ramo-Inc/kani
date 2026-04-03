//! Clipboard sync state — echo suppression for continuous clipboard sharing.

use kani_proto::clipboard::ClipboardMessage;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const ECHO_SUPPRESSION_TTL: Duration = Duration::from_millis(2000);
const MAX_ECHO_ENTRIES: usize = 8;

/// Clipboard sync manager with echo suppression.
///
/// When remote clipboard content is written locally, the OS clipboard monitor
/// detects the change and would re-send it. The `recent_remote_writes` deque
/// tracks recent remote writes to suppress these echoes.
pub struct ClipboardSync {
    local_content: Option<ClipboardMessage>,
    recent_remote_writes: VecDeque<(String, Instant)>,
}

impl ClipboardSync {
    pub fn new() -> Self {
        Self {
            local_content: None,
            recent_remote_writes: VecDeque::new(),
        }
    }

    pub fn on_local_change(&mut self, content: ClipboardMessage) -> bool {
        let now = Instant::now();
        self.recent_remote_writes
            .retain(|(_, ts)| now.duration_since(*ts) < ECHO_SUPPRESSION_TTL);

        let ClipboardMessage::Text(ref text) = content;
        if let Some(pos) = self
            .recent_remote_writes
            .iter()
            .position(|(t, _)| t == text)
        {
            self.recent_remote_writes.remove(pos);
            return false;
        }
        self.local_content = Some(content);
        true
    }

    pub fn get_local_content(&self) -> Option<&ClipboardMessage> {
        self.local_content.as_ref()
    }

    /// Record a remote write for echo suppression.
    /// Call this BEFORE writing to the OS clipboard.
    pub fn record_remote_write(&mut self, content: &ClipboardMessage) {
        let ClipboardMessage::Text(ref text) = content;
        self.recent_remote_writes
            .retain(|(_, ts)| ts.elapsed() < ECHO_SUPPRESSION_TTL);
        if self.recent_remote_writes.len() >= MAX_ECHO_ENTRIES {
            self.recent_remote_writes.pop_front();
        }
        self.recent_remote_writes
            .push_back((text.clone(), Instant::now()));
    }

    pub fn clear(&mut self) {
        self.local_content = None;
        self.recent_remote_writes.clear();
    }
}

impl Default for ClipboardSync {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_genuine_local_change() {
        let mut sync = ClipboardSync::new();
        assert!(sync.on_local_change(ClipboardMessage::Text("hello".into())));
        assert_eq!(
            sync.get_local_content(),
            Some(&ClipboardMessage::Text("hello".into()))
        );
    }

    #[test]
    fn test_echo_suppression_single() {
        let mut sync = ClipboardSync::new();
        sync.record_remote_write(&ClipboardMessage::Text("remote".into()));
        // Echoed back from OS monitor — should be suppressed
        assert!(!sync.on_local_change(ClipboardMessage::Text("remote".into())));
    }

    #[test]
    fn test_echo_suppression_rapid_writes() {
        let mut sync = ClipboardSync::new();
        sync.record_remote_write(&ClipboardMessage::Text("first".into()));
        sync.record_remote_write(&ClipboardMessage::Text("second".into()));

        assert!(!sync.on_local_change(ClipboardMessage::Text("first".into())));
        assert!(!sync.on_local_change(ClipboardMessage::Text("second".into())));
    }

    #[test]
    fn test_echo_entry_consumed_after_match() {
        let mut sync = ClipboardSync::new();
        sync.record_remote_write(&ClipboardMessage::Text("data".into()));
        assert!(!sync.on_local_change(ClipboardMessage::Text("data".into())));
        // Same content again — entry was consumed, so this is a genuine change
        assert!(sync.on_local_change(ClipboardMessage::Text("data".into())));
    }

    #[test]
    fn test_clear_resets_echo() {
        let mut sync = ClipboardSync::new();
        sync.record_remote_write(&ClipboardMessage::Text("data".into()));
        sync.on_local_change(ClipboardMessage::Text("local".into()));
        sync.clear();
        assert!(sync.get_local_content().is_none());
        // After clear, "data" is no longer suppressed
        assert!(sync.on_local_change(ClipboardMessage::Text("data".into())));
    }

    #[test]
    fn test_max_entries_eviction() {
        let mut sync = ClipboardSync::new();
        for i in 0..10 {
            sync.record_remote_write(&ClipboardMessage::Text(format!("msg{i}")));
        }
        // First 2 should have been evicted (10 - MAX_ECHO_ENTRIES = 2)
        assert!(sync.on_local_change(ClipboardMessage::Text("msg0".into())));
        assert!(sync.on_local_change(ClipboardMessage::Text("msg1".into())));
        // msg2 onwards should still be tracked
        assert!(!sync.on_local_change(ClipboardMessage::Text("msg2".into())));
    }
}
