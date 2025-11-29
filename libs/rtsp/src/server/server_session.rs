use std::net::SocketAddr;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct ServerSession {
    pub id: String,
    pub addr: SocketAddr,
    pub created_at: Instant,
    pub last_active: Instant,
    pub timeout: u64,
}

impl ServerSession {
    pub fn new(id: String, addr: SocketAddr, timeout: u64) -> Self {
        let now = Instant::now();
        Self {
            id,
            addr,
            created_at: now,
            last_active: now,
            timeout,
        }
    }

    pub fn update_activity(&mut self) {
        self.last_active = Instant::now();
    }

    pub fn is_expired(&self, now: Instant) -> bool {
        now.duration_since(self.last_active).as_secs() > self.timeout
    }

    pub fn age(&self) -> u64 {
        Instant::now().duration_since(self.created_at).as_secs()
    }

    pub fn idle_time(&self) -> u64 {
        Instant::now().duration_since(self.last_active).as_secs()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_session_creation() {
        let addr: SocketAddr = "127.0.0.1:8554".parse().unwrap();
        let session = ServerSession::new("test-id".to_string(), addr, 60);

        assert_eq!(session.id, "test-id");
        assert_eq!(session.addr, addr);
        assert_eq!(session.timeout, 60);
        assert!(!session.is_expired(Instant::now()));
    }

    #[test]
    fn test_session_activity_update() {
        let addr: SocketAddr = "127.0.0.1:8554".parse().unwrap();
        let mut session = ServerSession::new("test-id".to_string(), addr, 60);

        let initial_active = session.last_active;
        thread::sleep(Duration::from_millis(10));
        session.update_activity();

        assert!(session.last_active > initial_active);
    }

    #[test]
    fn test_session_expiration() {
        let addr: SocketAddr = "127.0.0.1:8554".parse().unwrap();
        let session = ServerSession::new("test-id".to_string(), addr, 0);

        thread::sleep(Duration::from_millis(10));
        assert!(session.is_expired(Instant::now()));
    }

    #[test]
    fn test_session_age() {
        let addr: SocketAddr = "127.0.0.1:8554".parse().unwrap();
        let session = ServerSession::new("test-id".to_string(), addr, 60);

        thread::sleep(Duration::from_millis(100));
        assert!(session.age() > 0);
    }

    #[test]
    fn test_idle_time() {
        let addr: SocketAddr = "127.0.0.1:8554".parse().unwrap();
        let mut session = ServerSession::new("test-id".to_string(), addr, 60);

        thread::sleep(Duration::from_millis(100));
        let idle_before = session.idle_time();

        session.update_activity();
        let idle_after = session.idle_time();

        assert!(idle_before > idle_after);
    }
}
