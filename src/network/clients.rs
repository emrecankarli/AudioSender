//! Server-side table of connected clients.
//!
//! The server fans every encoded frame out to every registered client, so the
//! size of this table multiplies the bandwidth it puts on the wire: `N` entries
//! cost `N` copies of the stream (21 KB/s per copy at the default settings).
//! Without a liveness check the table only ever grew — each client reconnection
//! binds a fresh ephemeral port, so a client that reconnected a few dozen times
//! left a few dozen dead endpoints behind, each still receiving every frame.
//! That is how a 21 KB/s stream turned into ~1.4 MB/s of traffic aimed at ports
//! nobody was listening on.
//!
//! Two invariants keep the fan-out bounded:
//!
//! * **One endpoint per host.** A handshake from an already-known host replaces
//!   that host's previous endpoint instead of adding a second one, so repeated
//!   reconnects can never grow the table.
//! * **Liveness.** Entries that have not sent a handshake within `timeout` are
//!   dropped. Clients re-send the handshake every second as a keep-alive, so a
//!   live client refreshes its entry long before it expires.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

/// Upper bound on distinct hosts.
///
/// With one endpoint per host this is also the upper bound on the fan-out, which
/// protects the link from a misbehaving or hostile sender on the LAN.
pub const MAX_CLIENTS: usize = 8;

/// How a handshake affected the table. Used for logging and by the tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Touch {
    /// A host we had not seen before.
    New,
    /// Known host, same endpoint: a keep-alive.
    Refreshed,
    /// Known host that came back on a different endpoint (restart or rebind).
    Moved { from: SocketAddr },
    /// Table is full and this host is not part of it.
    Rejected,
}

#[derive(Clone, Copy, Debug)]
struct Endpoint {
    addr: SocketAddr,
    last_seen: Instant,
}

pub struct ClientRegistry {
    clients: HashMap<IpAddr, Endpoint>,
    timeout: Duration,
}

impl ClientRegistry {
    pub fn new(timeout: Duration) -> Self {
        Self {
            clients: HashMap::new(),
            timeout,
        }
    }

    /// Register or refresh the endpoint a handshake came from.
    ///
    /// `now` is passed in rather than read from the clock so the expiry logic
    /// can be tested without sleeping.
    pub fn touch(&mut self, addr: SocketAddr, now: Instant) -> Touch {
        let host = addr.ip();

        if let Some(entry) = self.clients.get_mut(&host) {
            let from = entry.addr;
            entry.addr = addr;
            entry.last_seen = now;
            return if from == addr {
                Touch::Refreshed
            } else {
                Touch::Moved { from }
            };
        }

        if self.clients.len() >= MAX_CLIENTS {
            return Touch::Rejected;
        }

        self.clients.insert(
            host,
            Endpoint {
                addr,
                last_seen: now,
            },
        );
        Touch::New
    }

    /// Drop endpoints that have gone silent. Returns how many were removed.
    pub fn prune(&mut self, now: Instant) -> usize {
        let timeout = self.timeout;
        let before = self.clients.len();
        self.clients
            .retain(|_, entry| now.saturating_duration_since(entry.last_seen) < timeout);
        before - self.clients.len()
    }

    /// Endpoints that should receive the next frame. Sorted for stable logs.
    pub fn active(&self) -> Vec<SocketAddr> {
        let mut addrs: Vec<SocketAddr> = self.clients.values().map(|e| e.addr).collect();
        addrs.sort();
        addrs
    }

    pub fn len(&self) -> usize {
        self.clients.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddr};

    const TIMEOUT: Duration = Duration::from_secs(5);

    fn addr(host: [u8; 4], port: u16) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::from(host), port))
    }

    fn registry() -> (ClientRegistry, Instant) {
        (ClientRegistry::new(TIMEOUT), Instant::now())
    }

    #[test]
    fn first_handshake_adds_the_host() {
        let (mut reg, t0) = registry();
        assert_eq!(reg.touch(addr([192, 168, 31, 6], 51849), t0), Touch::New);
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.active(), vec![addr([192, 168, 31, 6], 51849)]);
    }

    #[test]
    fn keep_alive_does_not_grow_the_table() {
        let (mut reg, t0) = registry();
        let client = addr([192, 168, 31, 6], 51849);

        reg.touch(client, t0);
        // 60 keep-alives over a minute: still one entry.
        for second in 1..=60 {
            let now = t0 + Duration::from_secs(second);
            assert_eq!(reg.touch(client, now), Touch::Refreshed);
            assert_eq!(reg.prune(now), 0);
        }
        assert_eq!(reg.len(), 1);
    }

    /// This is the regression that mattered: a reconnecting client used to bind a
    /// new ephemeral port every attempt, and each one was kept forever, so every
    /// frame ended up being sent - and paid for on the wire - N times over.
    #[test]
    fn reconnect_from_a_new_port_replaces_the_old_endpoint() {
        let (mut reg, t0) = registry();
        let host = [192, 168, 31, 6];
        let old = addr(host, 51849);
        let new = addr(host, 52034);

        reg.touch(old, t0);
        assert_eq!(
            reg.touch(new, t0 + Duration::from_secs(1)),
            Touch::Moved { from: old }
        );

        assert_eq!(reg.len(), 1, "one host must never occupy two slots");
        assert_eq!(
            reg.active(),
            vec![new],
            "only the newest endpoint gets frames"
        );
    }

    #[test]
    fn repeated_reconnects_stay_bounded() {
        let (mut reg, t0) = registry();
        let host = [192, 168, 31, 6];

        // 57 reconnects, i.e. the number of stale ports observed in the wild.
        for port in 0..57u16 {
            reg.touch(
                addr(host, 49152 + port),
                t0 + Duration::from_secs(port as u64),
            );
        }
        assert_eq!(reg.len(), 1);
        assert_eq!(reg.active().len(), 1);
    }

    #[test]
    fn silent_endpoints_expire() {
        let (mut reg, t0) = registry();
        reg.touch(addr([192, 168, 31, 6], 51849), t0);

        assert_eq!(reg.prune(t0 + TIMEOUT - Duration::from_millis(1)), 0);
        assert_eq!(
            reg.prune(t0 + TIMEOUT),
            1,
            "entry must be gone at the timeout"
        );
        assert_eq!(reg.len(), 0);
        assert!(reg.active().is_empty());
    }

    #[test]
    fn expiry_is_per_endpoint() {
        let (mut reg, t0) = registry();
        let chatty = addr([192, 168, 31, 6], 50000);
        let silent = addr([192, 168, 31, 7], 50001);

        reg.touch(chatty, t0);
        reg.touch(silent, t0);
        reg.touch(chatty, t0 + Duration::from_secs(4));

        let now = t0 + Duration::from_secs(6);
        assert_eq!(reg.prune(now), 1);
        assert_eq!(
            reg.active(),
            vec![chatty],
            "only the silent host is dropped"
        );
    }

    #[test]
    fn distinct_hosts_are_capped() {
        let (mut reg, t0) = registry();
        for host in 0..MAX_CLIENTS as u8 {
            assert_eq!(reg.touch(addr([10, 0, 0, host], 50000), t0), Touch::New);
        }
        assert_eq!(reg.len(), MAX_CLIENTS);

        assert_eq!(
            reg.touch(addr([10, 0, 0, 200], 50000), t0),
            Touch::Rejected,
            "a full table must not grow"
        );
        assert_eq!(reg.len(), MAX_CLIENTS);

        // Hosts already in the table are unaffected by the cap.
        assert_eq!(
            reg.touch(addr([10, 0, 0, 1], 50000), t0 + Duration::from_secs(1)),
            Touch::Refreshed
        );
    }

    #[test]
    fn a_rejected_host_is_admitted_once_space_frees_up() {
        let (mut reg, t0) = registry();
        for host in 0..MAX_CLIENTS as u8 {
            reg.touch(addr([10, 0, 0, host], 50000), t0);
        }
        let late = addr([10, 0, 0, 200], 50000);
        assert_eq!(reg.touch(late, t0), Touch::Rejected);

        let later = t0 + TIMEOUT;
        assert_eq!(reg.prune(later), MAX_CLIENTS);
        assert_eq!(reg.touch(late, later), Touch::New);
    }
}
