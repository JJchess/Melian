use std::time::{Duration, Instant};

/// One notification per sustained fault episode, with no startup-wait noise.
#[derive(Default)]
pub struct Alerts {
    healthy_seen: bool,
    fault: Option<(String, Instant)>,
    notified: Option<String>,
}
impl Alerts {
    pub fn update(&mut self, allowed: bool, blocked: bool, reason: &str, now: Instant) -> bool {
        if allowed {
            self.healthy_seen = true;
            self.fault = None;
            self.notified = None;
            return false;
        }
        if !blocked {
            self.fault = None;
            return false;
        }
        if self.fault.as_ref().is_none_or(|(code, _)| code != reason) {
            self.fault = Some((reason.into(), now));
        }
        if self.healthy_seen
            && self.notified.as_deref() != Some(reason)
            && self
                .fault
                .as_ref()
                .is_some_and(|(_, start)| now.duration_since(*start) >= Duration::from_secs(2))
        {
            self.notified = Some(reason.into());
            return true;
        }
        false
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn startup_is_quiet_and_only_sustained_faults_notify() {
        let now = Instant::now();
        let mut a = Alerts::default();
        assert!(!a.update(false, true, "clashUnavailable", now));
        assert!(!a.update(
            false,
            true,
            "clashUnavailable",
            now + Duration::from_secs(10)
        ));
        assert!(!a.update(true, false, "", now + Duration::from_secs(11)));
        assert!(!a.update(false, true, "mode", now + Duration::from_secs(12)));
        assert!(!a.update(false, true, "mode", now + Duration::from_secs(13)));
        assert!(a.update(false, true, "mode", now + Duration::from_secs(14)));
        assert!(!a.update(false, true, "mode", now + Duration::from_secs(40)));
    }
    #[test]
    fn checking_cannot_duplicate_an_alert_but_recovery_rearms_it() {
        let now = Instant::now();
        let mut a = Alerts::default();
        a.update(true, false, "", now);
        a.update(false, true, "mode", now);
        assert!(a.update(false, true, "mode", now + Duration::from_secs(2)));
        a.update(false, false, "", now + Duration::from_secs(3));
        a.update(false, true, "mode", now + Duration::from_secs(4));
        assert!(!a.update(false, true, "mode", now + Duration::from_secs(6)));
        a.update(true, false, "", now + Duration::from_secs(7));
        a.update(false, true, "mode", now + Duration::from_secs(8));
        assert!(a.update(false, true, "mode", now + Duration::from_secs(10)));
    }
}
