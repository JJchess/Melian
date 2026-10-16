use std::{
    net::{Shutdown, TcpStream},
    sync::{Arc, Mutex, Weak},
    time::{Duration, Instant},
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
}
impl Lease {
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
        }))
    }
}
impl Gate {
    pub fn allow(&self, port: u16, duration: Duration) -> bool {
        let mut g = self.0.lock().unwrap();
        if g.expired {
            return false;
        }
        if g.port != 0 && g.until <= Instant::now() {
            g.port = 0;
            g.expired = true;
            g.generation += 1;
            g.close();
            return false;
        }
        if port == 0 || duration.is_zero() {
            return false;
        }
        if g.port != port || g.until <= Instant::now() {
            g.close();
            g.revoked = CancellationToken::new();
            g.generation += 1;
        }
        g.port = port;
        g.until = Instant::now() + duration;
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
        if g.generation != permit.generation || g.port != permit.port || g.until <= Instant::now() {
            let _ = socket.shutdown(Shutdown::Both);
            return false;
        }
        g.sockets.retain(|s| s.strong_count() > 0);
        g.sockets.push(Arc::downgrade(socket));
        true
    }
    pub fn permit(&self) -> Option<Permit> {
        let g = self.0.lock().unwrap();
        (g.port > 0 && g.until > Instant::now()).then(|| Permit {
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
        if g.port == 0 || g.until > Instant::now() {
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
