//! Read-only live smoke check. Does not start Claude, threads, or change any proxy.
use guard_platform::codex::Monitor;
use std::time::{Duration, Instant};
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let seconds = std::env::args()
        .nth(1)
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(30)
        .min(300);
    let monitor = Monitor::new();
    let stop = tokio_util::sync::CancellationToken::new();
    monitor.start(stop.clone());
    monitor.start(stop.clone()); // Idempotence: should still have only one child.
    let start = Instant::now();
    let mut previous = String::new();
    while start.elapsed() < Duration::from_secs(seconds) {
        tokio::time::sleep(Duration::from_secs(1)).await;
        let state = monitor.state();
        let value=serde_json::json!({
            "availability":state.availability,"remaining":state.remaining(),
            "windows":state.windows.len(),"accountUpdated":state.account_updated_at.is_some(),
            "accountUpdatedAt":state.account_updated_at,
            "announcementUpdatedAt":state.announcement_updated_at,
            "announcementsUpdated":state.announcement_updated_at.is_some(),
            "announcementsStale":state.announcement_stale,"announcementsError":state.announcement_error,
            "elapsedSeconds":start.elapsed().as_secs()/10*10
        }).to_string();
        if value != previous {
            println!("{value}");
            previous = value;
        }
    }
    stop.cancel();
    tokio::time::sleep(Duration::from_millis(500)).await;
    Ok(())
}
