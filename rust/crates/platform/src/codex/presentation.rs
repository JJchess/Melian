use chrono::{DateTime, Local, Utc};
use guard_protocol::codex::CodexState;

/// Deterministic shape for native preview captures; never used by monitoring.
pub fn preview() -> CodexState {
    let now = Utc::now();
    CodexState {
        availability: "ready".into(),
        windows: vec![guard_protocol::codex::QuotaWindow {
            id: "codex:primary".into(),
            label: "每周".into(),
            used_percent: 97.,
            duration_minutes: Some(10080),
            resets_at: Some(now + chrono::Duration::hours(26)),
        }],
        account_updated_at: Some(now),
        announcement_updated_at: Some(now),
        announcement: Some(guard_protocol::codex::ResetAnnouncement {
            id: "1".into(),
            summary: "Global reset landing within the next 2 hours.".into(),
            url: "https://x.com/thsottiaux/status/1".into(),
            published_at: Some(now),
            expected_by: Some(now + chrono::Duration::hours(2)),
            timing_note: None,
            phase: "announced".into(),
            source: super::announcements::SOURCE.into(),
        }),
        ..Default::default()
    }
}

pub fn menu(state: Option<&CodexState>) -> String {
    let Some(s) = state else {
        return "Codex · 尚未连接".into();
    };
    let status = if let Some(p) = s.remaining() {
        format!("剩余 {p}%")
    } else {
        match s.availability.as_str() {
            "checking" => "检查中",
            "notInstalled" => "未安装",
            "signedOut" => "未登录",
            "unsupported" => "额度不可用",
            _ => "暂无法更新",
        }
        .into()
    };
    format!("Codex · {status}{}", if s.unread { "  •" } else { "" })
}
pub fn countdown(at: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let seconds = (at - now).num_seconds();
    if seconds <= 0 {
        return "等待更新".into();
    }
    let minutes = (seconds + 59) / 60;
    if minutes >= 1440 {
        format!("约 {} 天 {} 小时后", minutes / 1440, minutes % 1440 / 60)
    } else if minutes >= 60 {
        format!("约 {} 小时 {} 分钟后", minutes / 60, minutes % 60)
    } else {
        format!("约 {minutes} 分钟后")
    }
}
pub struct Details {
    pub quota: String,
    pub reset: String,
    pub checked: String,
    pub pages: usize,
    pub announcement: String,
    pub published: String,
    pub source_checked: String,
    pub summary: String,
    pub expected: String,
    pub post: Option<String>,
}
pub fn details(state: Option<&CodexState>, page: usize, now: DateTime<Utc>) -> Details {
    let default = CodexState::default();
    let s = state.unwrap_or(&default);
    let window = s.windows.get(page % s.windows.len().max(1));
    let quota = window
        .map(|w| {
            format!(
                "{} · 剩余 {:.0}%{}",
                w.label,
                (100. - w.used_percent).clamp(0., 100.),
                if s.remaining().is_none() {
                    "（上次）"
                } else {
                    ""
                }
            )
        })
        .unwrap_or_else(|| {
            if s.message.is_empty() {
                "尚未连接 Codex".into()
            } else {
                s.message.clone()
            }
        });
    let reset = window
        .and_then(|w| w.resets_at)
        .map(|t| {
            format!(
                "{} · {}",
                t.with_timezone(&Local).format("%m/%d %H:%M"),
                countdown(t, now)
            )
        })
        .unwrap_or_else(|| "重置时间暂不可用".into());
    let checked = if s.availability == "ready" && !s.account_stale {
        "已更新"
    } else {
        "暂无法更新"
    };
    let checked = format!(
        "{checked} · {}",
        s.account_updated_at
            .map(|t| t.with_timezone(&Local).format("%H:%M").to_string())
            .unwrap_or_else(|| "—".into())
    );
    let announcement = if s.announcement_stale {
        "Tibo · 来源暂无法更新"
    } else {
        "Tibo · 重置预告转述"
    }
    .into();
    let summary = s
        .announcement
        .as_ref()
        .map(|a| {
            let text = a.summary.replace("https://t.co/", " ");
            let limit = 100;
            if text.chars().count() > limit {
                format!("{}…", text.chars().take(limit).collect::<String>())
            } else {
                text
            }
        })
        .unwrap_or_else(|| {
            if s.announcement_stale {
                "公告暂时无法更新".into()
            } else {
                "来源中未发现明确预告".into()
            }
        });
    let expected = s
        .announcement
        .as_ref()
        .map(|a| {
            let value = match a.phase.as_str() {
                "cancelled" => "该重置预告已取消".into(),
                "delayed" => "重置已延期 · 时间待公布".into(),
                "reportedComplete" => "Tibo 已报告完成 · 以账号为准".into(),
                "communityReported" => "社区报告已重置 · 以账号为准".into(),
                "awaitingConfirmation" => "预计时段已过 · 等待确认".into(),
                _ => a
                    .expected_by
                    .map(|t| {
                        format!(
                            "预计 {} 前 · {}",
                            t.with_timezone(&Local).format("%m/%d %H:%M"),
                            countdown(t, now)
                        )
                    })
                    .unwrap_or_else(|| {
                        a.timing_note.clone().unwrap_or_else(|| "时间待公布".into())
                    }),
            };
            if s.announcement_stale {
                format!("上次预告 · {value}")
            } else {
                value
            }
        })
        .unwrap_or_default();
    Details {
        quota,
        reset,
        checked,
        pages: s.windows.len().max(1),
        announcement,
        published: s
            .announcement
            .as_ref()
            .and_then(|a| a.published_at)
            .map(|t| format!("发布于 {}", t.with_timezone(&Local).format("%m/%d %H:%M")))
            .unwrap_or_else(|| "仅收录来源中的重置相关公告".into()),
        source_checked: s
            .announcement_source_updated_at
            .or(s.announcement_updated_at)
            .map(|t| format!("来源 {}", t.with_timezone(&Local).format("%m/%d %H:%M")))
            .unwrap_or_else(|| "来源尚未更新".into()),
        summary,
        expected,
        post: s
            .announcement
            .as_ref()
            .map(|a| a.url.clone())
            .filter(|u| super::announcements::safe_post(u)),
    }
}

/// A notification is never allowed to displace the network guard's fault UI.
#[derive(Default)]
pub struct NotificationCursor {
    seen: u64,
}
impl NotificationCursor {
    pub fn should_peek(
        &mut self,
        state: Option<&CodexState>,
        network_allowed: bool,
        expanded: bool,
    ) -> bool {
        let Some(s) = state else { return false };
        if !s.unread || s.notification_id == 0 || s.notification_id == self.seen {
            return false;
        }
        if !network_allowed || expanded {
            return false;
        }
        self.seen = s.notification_id;
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn network_fault_and_interaction_win_over_peek() {
        let mut c = NotificationCursor::default();
        let s = CodexState {
            unread: true,
            notification_id: 1,
            ..Default::default()
        };
        assert!(!c.should_peek(Some(&s), false, false));
        assert!(!c.should_peek(Some(&s), true, true));
        assert!(c.should_peek(Some(&s), true, false));
        assert!(!c.should_peek(Some(&s), true, false));
    }
    #[test]
    fn countdown_never_claims_completion() {
        let now = Utc::now();
        assert_eq!(countdown(now, now), "等待更新");
        assert_eq!(
            countdown(now + chrono::Duration::seconds(61), now),
            "约 2 分钟后"
        );
    }
    #[test]
    fn announcement_age_and_ambiguity_are_visible_separately_from_poll() {
        let mut s = preview();
        let now = Utc::now();
        let a = s.announcement.as_mut().unwrap();
        a.published_at = Some(now - chrono::Duration::hours(2));
        a.expected_by = None;
        a.timing_note = Some("时间表述有歧义 · 请查看原帖".into());
        let d = details(Some(&s), 0, now);
        assert!(d.published.starts_with("发布于 "));
        assert!(d.expected.contains("有歧义"));
        assert!(d.source_checked.starts_with("来源 "));
        s.announcement = None;
        assert_eq!(details(Some(&s), 0, now).summary, "来源中未发现明确预告");
        s.announcement_stale = true;
        assert_eq!(details(Some(&s), 0, now).summary, "公告暂时无法更新");
    }
}
