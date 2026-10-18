//! Read-only Codex information. None of these fields grants network access.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct QuotaWindow {
    pub id: String,
    pub label: String,
    pub used_percent: f64,
    pub duration_minutes: Option<i64>,
    pub resets_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct ResetAnnouncement {
    pub id: String,
    pub summary: String,
    pub url: String,
    pub published_at: Option<DateTime<Utc>>,
    /// Only a reliably parsed explicit time; never a historical prediction.
    pub expected_by: Option<DateTime<Utc>>,
    pub phase: String,
    pub source: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct CodexState {
    pub availability: String,
    pub message: String,
    pub windows: Vec<QuotaWindow>,
    pub account_updated_at: Option<DateTime<Utc>>,
    pub account_stale: bool,
    pub announcement: Option<ResetAnnouncement>,
    pub announcement_updated_at: Option<DateTime<Utc>>,
    pub announcement_stale: bool,
    pub announcement_error: Option<String>,
    pub unread: bool,
    pub notification_id: u64,
    pub notification: String,
}

impl CodexState {
    pub fn freshened(mut self, now: DateTime<Utc>) -> Self {
        self.account_stale = stale(self.account_updated_at, now, 180);
        self.announcement_stale =
            self.announcement_error.is_some() || stale(self.announcement_updated_at, now, 300);
        if let Some(a) = &mut self.announcement {
            if a.phase == "announced" && a.expected_by.is_some_and(|t| t <= now) {
                a.phase = "awaitingConfirmation".into();
            }
        }
        self
    }
    pub fn remaining(&self) -> Option<u8> {
        if self.availability != "ready" || self.account_stale {
            return None;
        }
        self.windows
            .iter()
            .map(|w| (100. - w.used_percent).clamp(0., 100.).floor() as u8)
            .min()
    }
}

fn stale(at: Option<DateTime<Utc>>, now: DateTime<Utc>, seconds: i64) -> bool {
    at.is_none_or(|t| (now - t).num_seconds() > seconds || t > now)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_protocol_and_missing_codex_are_compatible() {
        let state: crate::State =
            serde_json::from_str(r#"{"status":"allowed","allowed":true}"#).unwrap();
        assert!(state.codex.is_none());
        assert!(!state.fresh_allowed());
    }
    #[test]
    fn stale_and_clock_rollback_cannot_report_remaining() {
        let now = Utc::now();
        let state = CodexState {
            availability: "ready".into(),
            windows: vec![QuotaWindow {
                used_percent: 97.,
                ..Default::default()
            }],
            account_updated_at: Some(now),
            ..Default::default()
        };
        assert_eq!(state.clone().freshened(now).remaining(), Some(3));
        assert_eq!(
            state
                .clone()
                .freshened(now + chrono::Duration::seconds(181))
                .remaining(),
            None
        );
        assert_eq!(
            state
                .freshened(now - chrono::Duration::seconds(1))
                .remaining(),
            None
        );
    }
}
