//! A one-use browser routing check. The nonce contains no account or session data.
//! Only a locally registered browser can be acknowledged, and the destination is
//! always the fixed Claude URL; an HTTP request cannot choose a redirect target.
use crate::gate::{Gate, Permit};
use std::{
    collections::BTreeMap,
    sync::Mutex,
    time::{Duration, Instant},
};

pub const PROBE_HOST: &str = "connect.claudeguard.invalid";
const LIFETIME: Duration = Duration::from_secs(60);

struct Entry {
    nonce: String,
    expires: Instant,
    consumed: bool,
    verified: Option<Instant>,
}

#[derive(Default)]
pub struct WebProbe(Mutex<BTreeMap<&'static str, Entry>>);

pub(crate) struct Receipt {
    browser: &'static str,
    nonce: String,
}

pub(crate) enum ProbeRequest {
    Other,
    NotFound,
    Redirect(Receipt),
}

fn browser_key(browser: &str) -> Option<&'static str> {
    if browser.eq_ignore_ascii_case("edge") {
        Some("edge")
    } else if browser.eq_ignore_ascii_case("chrome") {
        Some("chrome")
    } else {
        None
    }
}

impl WebProbe {
    /// Replaces this browser's previous challenge and acknowledgement. There are
    /// at most two registrations, one each for Chrome and Edge. Unknown browsers
    /// receive an unregistered URL which can never produce an acknowledgement.
    pub fn issue(&self, browser: &str) -> String {
        let nonce = uuid::Uuid::new_v4().to_string();
        if let Some(browser) = browser_key(browser) {
            self.0.lock().unwrap().insert(
                browser,
                Entry {
                    nonce: nonce.clone(),
                    expires: Instant::now() + LIFETIME,
                    consumed: false,
                    verified: None,
                },
            );
        }
        format!("http://{PROBE_HOST}/{nonce}")
    }

    /// The monotonic instant at which a successful local response was written.
    /// This proves that one browser request used the guard; callers must also
    /// check that the owned browser configuration has not changed since then.
    pub fn verified(&self, browser: &str) -> Option<Instant> {
        let browser = browser_key(browser)?;
        self.0.lock().unwrap().get(browser)?.verified
    }

    pub fn clear(&self, browser: &str) {
        if let Some(browser) = browser_key(browser) {
            self.0.lock().unwrap().remove(browser);
        }
    }

    /// Called only after the HTTP header is validated. Every request to the
    /// reserved host terminates locally, including CONNECT and bad nonces.
    pub(crate) fn consume_request(
        &self,
        header: &[u8],
        gate: &Gate,
        permit: &Permit,
    ) -> ProbeRequest {
        let Ok(header) = std::str::from_utf8(header) else {
            return ProbeRequest::Other;
        };
        let mut first = header.lines().next().unwrap_or("").split(' ');
        let method = first.next().unwrap_or("");
        let target = first.next().unwrap_or("");
        if method == "CONNECT" {
            return match target.parse::<hyper::http::uri::Authority>() {
                Ok(authority) if authority.host().eq_ignore_ascii_case(PROBE_HOST) => {
                    ProbeRequest::NotFound
                }
                _ => ProbeRequest::Other,
            };
        }
        let Ok(uri) = target.parse::<hyper::Uri>() else {
            return ProbeRequest::Other;
        };
        if !uri
            .host()
            .is_some_and(|host| host.eq_ignore_ascii_case(PROBE_HOST))
        {
            return ProbeRequest::Other;
        }
        if method != "GET"
            || uri.scheme_str() != Some("http")
            || !uri
                .authority()
                .is_some_and(|authority| authority.as_str().eq_ignore_ascii_case(PROBE_HOST))
            || uri.query().is_some()
            || !gate.current(permit)
        {
            return ProbeRequest::NotFound;
        }
        let nonce = uri.path().strip_prefix('/').unwrap_or("");
        let mut entries = self.0.lock().unwrap();
        let now = Instant::now();
        for (&browser, entry) in entries.iter_mut() {
            if entry.nonce == nonce && !entry.consumed && now < entry.expires {
                // Consuming before the write prevents parallel or replayed
                // requests from obtaining another redirect with this nonce.
                entry.consumed = true;
                return ProbeRequest::Redirect(Receipt {
                    browser,
                    nonce: entry.nonce.clone(),
                });
            }
        }
        ProbeRequest::NotFound
    }

    pub(crate) fn acknowledge(&self, receipt: Receipt, gate: &Gate, permit: &Permit) {
        let mut entries = self.0.lock().unwrap();
        let now = Instant::now();
        if let Some(entry) = entries.get_mut(receipt.browser) {
            if entry.nonce == receipt.nonce
                && entry.consumed
                && now < entry.expires
                && gate.current(permit)
            {
                entry.verified = Some(now);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(url: &str) -> Vec<u8> {
        format!("GET {url} HTTP/1.1\r\nHost: {PROBE_HOST}\r\n\r\n").into_bytes()
    }

    #[test]
    fn expired_challenge_cannot_acknowledge() {
        let probe = WebProbe::default();
        let url = probe.issue("chrome");
        probe.0.lock().unwrap().get_mut("chrome").unwrap().expires =
            Instant::now() - Duration::from_millis(1);
        let gate = Gate::default();
        gate.allow(7897, Duration::from_secs(10));
        let permit = gate.permit().unwrap();
        assert!(matches!(
            probe.consume_request(&request(&url), &gate, &permit),
            ProbeRequest::NotFound
        ));
        assert!(probe.verified("chrome").is_none());
    }

    #[test]
    fn replacing_or_clearing_challenge_invalidates_pending_acknowledgement() {
        let probe = WebProbe::default();
        let gate = Gate::default();
        gate.allow(7897, Duration::from_secs(10));
        let permit = gate.permit().unwrap();
        let url = probe.issue("chrome");
        let ProbeRequest::Redirect(receipt) = probe.consume_request(&request(&url), &gate, &permit)
        else {
            panic!("new challenge must be consumable");
        };
        let next = probe.issue("chrome");
        probe.acknowledge(receipt, &gate, &permit);
        assert!(probe.verified("chrome").is_none());
        assert!(matches!(
            probe.consume_request(&request(&url), &gate, &permit),
            ProbeRequest::NotFound
        ));
        probe.clear("chrome");
        assert!(matches!(
            probe.consume_request(&request(&next), &gate, &permit),
            ProbeRequest::NotFound
        ));
    }

    #[test]
    fn revocation_between_consumption_and_response_does_not_acknowledge() {
        let probe = WebProbe::default();
        let gate = Gate::default();
        gate.allow(7897, Duration::from_secs(10));
        let permit = gate.permit().unwrap();
        let url = probe.issue("edge");
        let ProbeRequest::Redirect(receipt) = probe.consume_request(&request(&url), &gate, &permit)
        else {
            panic!("new challenge must be consumable");
        };
        gate.revoke();
        gate.allow(7897, Duration::from_secs(10));
        probe.acknowledge(receipt, &gate, &permit);
        assert!(probe.verified("edge").is_none());
    }
}
