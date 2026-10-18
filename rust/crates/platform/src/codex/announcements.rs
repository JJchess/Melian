use chrono::{DateTime, Duration, Utc};
use guard_protocol::codex::ResetAnnouncement;
use serde_json::Value;

pub const SOURCE: &str = "https://codex-reset.com/";
pub fn safe_post(url: &str) -> bool {
    let Some(id) = url
        .strip_prefix("https://x.com/thsottiaux/status/")
        .or_else(|| url.strip_prefix("https://twitter.com/thsottiaux/status/"))
    else {
        return false;
    };
    !id.is_empty() && id.len() <= 30 && id.bytes().all(|b| b.is_ascii_digit())
}
fn time(value: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|v| v.with_timezone(&Utc))
}
fn clean(text: &str) -> String {
    text.replace("&amp;", "&")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn excluded(text: &str) -> bool {
    [
        "banked",
        "reset credit",
        "extra reset",
        "reset token",
        "manual reset",
        "would ",
        "could ",
        "might ",
        "hope ",
        "wish ",
        "jok",
        "rumor",
        "rumour",
        "not resetting",
        "no reset",
        "won't reset",
        "don't reset",
    ]
    .iter()
    .any(|s| text.contains(s))
        || text.contains('?')
}
fn explicit(text: &str) -> bool {
    !excluded(text)
        && (text.contains("global reset")
            || text.contains("reset usage")
            || text.contains("reset the usage")
            || text.contains("resetting usage")
            || text.contains("reset limits")
            || text.contains("resetting limits")
            || text.contains("reset of") && text.contains("codex")
            || text.contains("reset") && (text.contains("codex") || text.contains("usage limits"))
            || text.contains("day ") && text.contains("/ reset")
            || text.contains("reset")
                && (text.contains("it's done") || text.contains("all propagated")))
}
/// Intentionally conservative: ambiguous local dates / PST during DST are text only.
pub fn expected_by(text: &str, posted: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let text = text.to_lowercase();
    let words: Vec<_> = text.split_whitespace().collect();
    for (i, word) in words.iter().enumerate() {
        if *word != "within" && *word != "in" {
            continue;
        }
        let mut n = i + 1;
        if words.get(n) == Some(&"the") {
            n += 1;
        }
        if words.get(n) == Some(&"next") {
            n += 1;
        }
        let amount = match *words.get(n)? {
            "one" | "an" => 1,
            "two" => 2,
            "three" => 3,
            v => match v.parse::<i64>() {
                Ok(v) if (1..=72).contains(&v) => v,
                _ => continue,
            },
        };
        let unit = words.get(n + 1)?.trim_end_matches(['.', ',', '!']);
        let minutes = if unit.starts_with("hour") {
            amount * 60
        } else if unit.starts_with("minute") {
            amount
        } else {
            continue;
        };
        return Some(posted + Duration::minutes(minutes));
    }
    // Explicit full date and offset only; never invent a date from "tomorrow".
    for word in text.split_whitespace() {
        if let Ok(v) = DateTime::parse_from_rfc3339(&word.to_uppercase()) {
            return Some(v.with_timezone(&Utc));
        }
    }
    None
}

/// Feed classifications are advisory. Require explicit text from the allowlisted author,
/// and a parent reset announcement before interpreting a short timing reply.
pub fn parse(
    feed: &Value,
    timeline: &Value,
    now: DateTime<Utc>,
) -> anyhow::Result<Option<ResetAnnouncement>> {
    anyhow::ensure!(feed["stale"].as_bool() == Some(false), "公告来源暂未更新");
    let fetched = time(&feed["fetched_at"]).ok_or_else(|| anyhow::anyhow!("公告缺少更新时间"))?;
    anyhow::ensure!(
        (now - fetched).num_seconds() <= 300 && fetched <= now + Duration::minutes(2),
        "公告来源暂未更新"
    );
    let tweets = feed["tweets"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("公告格式已变化"))?;
    let mut found = Vec::new();
    for post in tweets.iter().take(500) {
        let Some(url) = post["url"].as_str().filter(|u| safe_post(u)) else {
            continue;
        };
        let Some(id) = post["id"].as_str() else {
            continue;
        };
        if !url.ends_with(&format!("/{id}")) {
            continue;
        }
        let Some(at) = time(&post["at"]) else {
            continue;
        };
        if at > now + Duration::minutes(2) || now - at > Duration::days(7) {
            continue;
        }
        let text = clean(post["text"].as_str().unwrap_or(""));
        let lower = text.to_lowercase();
        let parent = post["in_reply_to_tweet_id"]
            .as_str()
            .and_then(|parent_id| tweets.iter().find(|p| p["id"].as_str() == Some(parent_id)));
        let parent_reset = parent.is_some_and(|p| {
            safe_post(p["url"].as_str().unwrap_or(""))
                && explicit(&clean(p["text"].as_str().unwrap_or("")).to_lowercase())
        });
        let cancelled = lower.contains("cancelled")
            || lower.contains("canceled")
            || lower.contains("no reset")
            || lower.contains("not resetting");
        let delayed = lower.contains("postponed") || lower.contains("delayed");
        let correction = parent_reset
            && (cancelled || delayed || lower.contains("correction") || lower.contains("meant "));
        if excluded(&lower) && !correction {
            continue;
        }
        let completed = lower.contains("all propagated")
            || lower.contains("it's done")
            || lower.contains("reset complete")
            || lower.contains("confirmed landed");
        let reply_timing = parent_reset
            && (lower.contains("landing")
                || lower.contains("will land")
                || lower.contains("will be")
                || lower.contains("within")
                || completed);
        if !explicit(&lower) && !reply_timing && !correction {
            continue;
        }
        let expected = if cancelled || delayed {
            None
        } else {
            expected_by(&text, at)
        };
        let context = if reply_timing && !explicit(&lower) {
            "额度重置 · "
        } else {
            ""
        };
        let mut phase = if cancelled {
            "cancelled"
        } else if delayed {
            "delayed"
        } else if completed {
            "reportedComplete"
        } else if expected.is_some_and(|t| t <= now) {
            "awaitingConfirmation"
        } else {
            "announced"
        };
        // An operator-observed timeline event is a community report, not a Tibo completion.
        if !completed
            && timeline["events"].as_array().is_some_and(|events| {
                events.iter().any(|e| {
                    e["url"].as_str() == Some(url)
                        && e["group"] == "reset"
                        && e["summary"]
                            .as_str()
                            .is_some_and(|s| s.to_lowercase().contains("observed by operator"))
                })
            })
        {
            phase = "communityReported";
        }
        // Undated old previews stop looking like upcoming events after 24 hours.
        if phase == "announced"
            && expected.is_none()
            && !completed
            && now - at > Duration::hours(24)
        {
            phase = "awaitingConfirmation";
        }
        found.push(ResetAnnouncement {
            id: id.into(),
            summary: format!("{context}{}", text.chars().take(280).collect::<String>()),
            url: url.into(),
            published_at: Some(at),
            expected_by: expected,
            phase: phase.into(),
            source: SOURCE.into(),
        });
    }
    found.sort_by_key(|a| a.published_at);
    Ok(found.pop())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn feed(posts: Value, now: DateTime<Utc>) -> Value {
        json!({"stale":false,"fetched_at":now,"tweets":posts})
    }
    fn post(id: &str, text: &str, at: DateTime<Utc>) -> Value {
        json!({"id":id,"url":format!("https://x.com/thsottiaux/status/{id}"),"text":text,"at":at})
    }
    #[test]
    fn explicit_relative_and_corrected_reply() {
        let now = Utc::now();
        let parent = post(
            "1",
            "The reports of my reset button being lost are greatly exaggerated Day 10/ Reset",
            now - Duration::minutes(2),
        );
        let mut reply = post("2", "Landing within the next 2 hours", now);
        reply["in_reply_to_tweet_id"] = json!("1");
        let a = parse(&feed(json!([parent, reply]), now), &json!({}), now)
            .unwrap()
            .unwrap();
        assert_eq!(a.id, "2");
        assert_eq!(a.expected_by, Some(now + Duration::hours(2)));
    }
    #[test]
    fn unsupported_context_is_not_a_countdown() {
        let now = Utc::now();
        for text in [
            "Landing within the next 2 hours",
            "I hope Codex will reset",
            "Could we reset Codex?",
            "Loading a banked reset in paid accounts",
            "Let's break the cycle",
            "No reset today",
        ] {
            assert!(
                parse(&feed(json!([post("1", text, now)]), now), &json!({}), now)
                    .unwrap()
                    .is_none(),
                "{text}"
            );
        }
        for text in [
            "Global reset by EOD PST",
            "Global reset tomorrow at 2pm PST",
            "Global reset soon",
        ] {
            let a = parse(&feed(json!([post("1", text, now)]), now), &json!({}), now)
                .unwrap()
                .unwrap();
            assert!(a.expected_by.is_none());
        }
    }
    #[test]
    fn cancellation_and_delay_replace_earlier_preview() {
        let now = Utc::now();
        for (text, phase) in [
            ("Cancelled, no reset today", "cancelled"),
            ("Delayed until further notice", "delayed"),
        ] {
            let parent = post(
                "1",
                "Global reset within two hours",
                now - Duration::minutes(1),
            );
            let mut reply = post("2", text, now);
            reply["in_reply_to_tweet_id"] = json!("1");
            let a = parse(&feed(json!([parent, reply]), now), &json!({}), now)
                .unwrap()
                .unwrap();
            assert_eq!(a.id, "2");
            assert_eq!(a.phase, phase);
            assert!(a.expected_by.is_none());
        }
    }
    #[test]
    fn expired_stale_and_untrusted_sources() {
        let now = Utc::now();
        let a = parse(
            &feed(
                json!([post(
                    "1",
                    "Global reset within 1 hour",
                    now - Duration::hours(2)
                )]),
                now,
            ),
            &json!({}),
            now,
        )
        .unwrap()
        .unwrap();
        assert_eq!(a.phase, "awaitingConfirmation");
        assert!(parse(
            &feed(json!([]), now - Duration::minutes(6)),
            &json!({}),
            now
        )
        .is_err());
        assert!(!safe_post("https://x.com/thsottiaux/status/123?next=evil"));
        assert!(!safe_post("https://x.com.evil/thsottiaux/status/123"));
    }
}
