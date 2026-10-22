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
fn normalized(text: &str) -> String {
    clean(text).to_lowercase().replace('’', "'")
}
fn series_reset(text: &str) -> bool {
    // The daily series often separates "Day N/" and "reset" with other words.
    // Do not promote every mention of reset in that series into a promise.
    let words: Vec<_> = text.split_whitespace().collect();
    let day = words.windows(2).any(|w| {
        w[0] == "day"
            && w[1]
                .strip_suffix('/')
                .is_some_and(|n| n.parse::<u16>().is_ok_and(|n| n > 0))
    });
    day && (text.contains("/ reset")
        || text.contains("limit reset")
        || text.contains("it's reset day")
        || text.contains("and reset. enjoy"))
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
        "asking for",
        "requesting",
        "demand ",
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
            || series_reset(text)
            || text.contains("reset")
                && (text.contains("it's done") || text.contains("all propagated")))
}
fn valid_post(post: &Value, now: DateTime<Utc>) -> Option<(&str, &str, DateTime<Utc>)> {
    let url = post["url"].as_str().filter(|u| safe_post(u))?;
    let id = post["id"].as_str()?;
    let at = time(&post["at"])?;
    (url.ends_with(&format!("/{id}"))
        && at <= now + Duration::minutes(2)
        && now - at <= Duration::days(7))
    .then_some((id, url, at))
}
fn completion(text: &str) -> bool {
    text.contains("all propagated")
        || text.contains("it's done")
        || text.contains("reset complete")
        || text.contains("confirmed landed")
        || text.contains("done and dusted")
        || text.trim_end_matches(['.', '!']) == "done"
}
fn timing_reply(text: &str) -> bool {
    [
        "landing",
        "lands ",
        "will land",
        "will be",
        "within",
        "correction",
        "meant ",
        "cancelled",
        "canceled",
        "no reset",
        "not resetting",
        "postponed",
        "delayed",
    ]
    .iter()
    .any(|p| text.contains(p))
        || completion(text)
}
fn parent_reset(post: &Value, tweets: &[Value], now: DateTime<Utc>) -> bool {
    let mut child = post;
    let mut seen = std::collections::HashSet::new();
    for _ in 0..8 {
        let Some(id) = child["in_reply_to_tweet_id"].as_str() else {
            return false;
        };
        if !seen.insert(id) {
            return false;
        }
        let Some(parent) = tweets
            .iter()
            .take(500)
            .find(|p| p["id"].as_str() == Some(id))
        else {
            return false;
        };
        let Some((_, _, at)) = valid_post(parent, now) else {
            return false;
        };
        if time(&child["at"]).is_none_or(|child_at| at > child_at) {
            return false;
        }
        let text = normalized(parent["text"].as_str().unwrap_or(""));
        if explicit(&text) {
            return true;
        }
        if !timing_reply(&text) {
            return false;
        }
        child = parent;
    }
    false
}
fn timing_note(text: &str, expected: Option<DateTime<Utc>>) -> Option<String> {
    (expected.is_none()
        && ["tomorrow", "today", "pst", "pdt", "eod", "before ", " at "]
            .iter()
            .any(|p| normalized(text).contains(p)))
    .then(|| "时间表述有歧义 · 请查看原帖".into())
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
        let Some((id, url, at)) = valid_post(post, now) else {
            continue;
        };
        let text = clean(post["text"].as_str().unwrap_or(""));
        let lower = normalized(&text);
        let parent_reset = parent_reset(post, tweets, now);
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
        let completed = completion(&lower);
        let reply_timing = parent_reset && timing_reply(&lower);
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
                        && e["banked_grant"] != true
                        && (e["confirmed"] == true
                            || e["summary"]
                                .as_str()
                                .is_some_and(|s| s.to_lowercase().contains("observed by operator")))
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
            timing_note: timing_note(&text, expected),
            phase: phase.into(),
            source: SOURCE.into(),
        });
    }
    found.sort_by_key(|a| a.published_at);
    Ok(found.pop())
}

pub fn source_updated_at(feed: &Value) -> Option<DateTime<Utc>> {
    time(&feed["fetched_at"])
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
    #[test]
    fn october_22_reset_day_and_lands_reply_are_not_lost() {
        let now = DateTime::parse_from_rfc3339("2026-10-22T07:01:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let parent = post(
            "2113162277688070445",
            "Day 17/ Bet you thought we forgot, but no, it’s reset day today.",
            now - Duration::minutes(6),
        );
        let mut reply = post(
            "2113162413008884099",
            "Lands tomorrow before 10am PST.",
            now - Duration::minutes(5),
        );
        reply["in_reply_to_tweet_id"] = json!("2113162277688070445");
        let a = parse(&feed(json!([reply, parent]), now), &json!({}), now)
            .unwrap()
            .unwrap();
        assert_eq!(a.id, "2113162413008884099");
        assert_eq!(a.phase, "announced");
        assert!(a.expected_by.is_none());
        assert!(a.timing_note.is_some());
        assert_eq!(a.published_at, Some(now - Duration::minutes(5)));
    }
    #[test]
    fn daily_variations_and_structured_community_confirmation() {
        let now = Utc::now();
        for text in [
            "Day 13/ Le limit reset",
            "Day 15/ Nevermind, we're still cooking... and reset. Enjoy.",
        ] {
            let timeline = json!({"events":[{"url":"https://x.com/thsottiaux/status/1", "group":"reset", "confirmed":true, "banked_grant":false}]});
            let a = parse(&feed(json!([post("1", text, now)]), now), &timeline, now)
                .unwrap()
                .unwrap();
            assert_eq!(a.phase, "communityReported");
        }
        for text in [
            "I demand a reset",
            "Day 17/ Can we get a reset?",
            "Day 17/ I hope it's reset day",
            "Day 17/ More reset credits",
            "Day 17/ We might reset Codex",
            "Day 17/ People are asking for a limit reset",
        ] {
            assert!(
                parse(&feed(json!([post("1", text, now)]), now), &json!({}), now)
                    .unwrap()
                    .is_none(),
                "{text}"
            );
        }
    }
    #[test]
    fn nested_replies_need_valid_context_and_newer_correction_wins() {
        let now = Utc::now();
        let parent = post("1", "Global reset in two hours", now - Duration::minutes(3));
        let mut reply = post(
            "2",
            "Lands tomorrow before 10am PST.",
            now - Duration::minutes(2),
        );
        reply["in_reply_to_tweet_id"] = json!("1");
        let mut correction = post("3", "Correction, lands within 3 hours", now);
        correction["in_reply_to_tweet_id"] = json!("2");
        let a = parse(
            &feed(json!([correction, reply, parent]), now),
            &json!({}),
            now,
        )
        .unwrap()
        .unwrap();
        assert_eq!(a.id, "3");
        assert_eq!(a.expected_by, Some(now + Duration::hours(3)));
        let mut orphan = post("4", "Lands within 2 hours", now);
        orphan["in_reply_to_tweet_id"] = json!("missing");
        assert!(parse(&feed(json!([orphan]), now), &json!({}), now)
            .unwrap()
            .is_none());
    }
    #[test]
    fn completion_reply_does_not_claim_account_recovery() {
        let now = Utc::now();
        let parent = post("1", "Day 13/ Le limit reset", now - Duration::minutes(1));
        let mut reply = post("2", "Done.", now);
        reply["in_reply_to_tweet_id"] = json!("1");
        let a = parse(&feed(json!([parent, reply]), now), &json!({}), now)
            .unwrap()
            .unwrap();
        assert_eq!(a.phase, "reportedComplete");
        assert!(a.expected_by.is_none());
    }
}
