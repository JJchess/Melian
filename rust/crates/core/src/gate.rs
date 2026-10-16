use std::{
    net::{Shutdown, TcpStream},
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant, SystemTime},
};
use tokio_util::sync::CancellationToken;
#[derive(Clone)]
pub struct Permit {
    pub port: u16,
    pub generation: u64,
    pub revoked: CancellationToken,
}
struct Lease {
    port: u16,
    until: Instant,
    generation: u64,
    revoked: CancellationToken,
    sockets: Vec<Weak<TcpStream>>,
    expired: bool,
    wall_issued: SystemTime,
    duration: Duration,
}
impl Lease {
    fn valid(&self) -> bool {
        // Instant remains authoritative. The additional clock only tightens the
        // lease across macOS sleep (mach_absolute_time can pause while asleep).
        self.until > Instant::now()
            && self
                .wall_issued
                .elapsed()
                .is_ok_and(|elapsed| elapsed < self.duration)
    }
    fn close(&mut self) {
        self.revoked.cancel();
        for socket in self.sockets.drain(..).filter_map(|s| s.upgrade()) {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }
}
pub struct Gate(Mutex<Lease>);
impl Default for Gate {
    fn default() -> Self {
        Self(Mutex::new(Lease {
            port: 0,
            until: Instant::now(),
            generation: 0,
            revoked: CancellationToken::new(),
            sockets: vec![],
            expired: false,
            wall_issued: SystemTime::now(),
            duration: Duration::ZERO,
        }))
    }
}
impl Gate {
    pub fn allow(&self, port: u16, duration: Duration) -> bool {
        let mut g = self.0.lock().unwrap();
        if g.expired {
            return false;
        }
        if g.port != 0 && !g.valid() {
            g.port = 0;
            g.expired = true;
            g.generation += 1;
            g.close();
            return false;
        }
        if port == 0 || duration.is_zero() {
            return false;
        }
        if g.port != port || !g.valid() {
            g.close();
            g.revoked = CancellationToken::new();
            g.generation += 1;
        }
        g.port = port;
        g.until = Instant::now() + duration;
        g.wall_issued = SystemTime::now();
        g.duration = duration;
        true
    }
    pub fn revoke(&self) {
        let mut g = self.0.lock().unwrap();
        g.port = 0;
        g.expired = false;
        g.until = Instant::now();
        g.generation += 1;
        g.close();
        g.revoked = CancellationToken::new();
    }
    pub fn track(&self, permit: &Permit, socket: &Arc<TcpStream>) -> bool {
        let mut g = self.0.lock().unwrap();
        if g.generation != permit.generation || g.port != permit.port || !g.valid() {
            let _ = socket.shutdown(Shutdown::Both);
            return false;
        }
        g.sockets.retain(|s| s.strong_count() > 0);
        g.sockets.push(Arc::downgrade(socket));
        true
    }
    pub fn permit(&self) -> Option<Permit> {
        let g = self.0.lock().unwrap();
        (g.port > 0 && g.valid()).then(|| Permit {
            port: g.port,
            generation: g.generation,
            revoked: g.revoked.clone(),
        })
    }
    pub fn current(&self, p: &Permit) -> bool {
        self.permit()
            .is_some_and(|now| now.generation == p.generation && now.port == p.port)
    }
    pub fn expire(&self) -> bool {
        let mut g = self.0.lock().unwrap();
        if g.expired {
            return true;
        }
        if g.port == 0 || g.valid() {
            return false;
        }
        g.port = 0;
        g.expired = true;
        g.generation += 1;
        g.close();
        g.revoked = CancellationToken::new();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleep_cannot_extend_a_lease_or_revive_its_generation() {
        let gate = Gate::default();
        assert!(gate.allow(7890, Duration::from_secs(45)));
        let permit = gate.permit().unwrap();
        gate.0.lock().unwrap().wall_issued = SystemTime::now() - Duration::from_secs(60);
        assert!(gate.permit().is_none());
        assert!(gate.expire());
        assert!(permit.revoked.is_cancelled());
        assert!(!gate.allow(7890, Duration::from_secs(45)));
        gate.revoke();
        assert!(gate.allow(7890, Duration::from_secs(45)));
        assert!(!gate.current(&permit));
    }

    #[test]
    fn clock_moved_backwards_fails_closed() {
        let gate = Gate::default();
        assert!(gate.allow(7890, Duration::from_secs(45)));
        gate.0.lock().unwrap().wall_issued = SystemTime::now() + Duration::from_secs(60);
        assert!(gate.permit().is_none());
        assert!(gate.expire());
    }
}
