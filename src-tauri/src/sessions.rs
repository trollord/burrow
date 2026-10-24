//! Session history: when you were connected, where to, and how much you moved.
//!
//! Byte counts come from the tunnel interface's own counters, read with `netstat -ibn`, which
//! needs no privileges. `wg show transfer` would be the obvious source and does require root —
//! asking for a password to read a statistic would be absurd.
//!
//! The counters are cumulative for the life of the interface, and `wg-quick` creates a fresh one
//! per connection, so the reading at disconnect *is* the session total with no bookkeeping.

use crate::paths::{config_dir, now, read_json, write_json};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Session {
    pub id: i64,
    pub region: String,
    pub endpoint: String,
    pub started_at: i64,
    /// None while the session is still open.
    pub ended_at: Option<i64>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

impl Session {
    pub fn total(&self) -> u64 {
        self.rx_bytes + self.tx_bytes
    }
}

#[derive(Serialize, Deserialize, Default)]
struct Store {
    sessions: Vec<Session>,
}

fn path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("sessions.json"))
}

/// The tunnel interface, found by the address Burrow assigns rather than by name — `wg-quick`
/// picks whichever `utunN` is free, so the number changes between connections.
pub fn tunnel_iface() -> Option<String> {
    let out = Command::new("/sbin/ifconfig").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut current = None;
    for line in text.lines() {
        if !line.starts_with(char::is_whitespace) {
            current = line.split(':').next().map(str::to_string);
        } else if line.trim_start().starts_with("inet 10.8.0.2") {
            return current;
        }
    }
    None
}

/// Cumulative (received, sent) bytes for an interface.
///
/// The `<Link#N>` row holds the interface total; the rows below it are per-address and would
/// double-count. That row has no Address field, so it is ten columns where the others are eleven,
/// which is why this matches on the marker rather than counting from the right.
pub fn counters(iface: &str) -> Option<(u64, u64)> {
    let out = Command::new("/usr/sbin/netstat").args(["-ibn"]).output().ok()?;
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() >= 9 && f[0] == iface && f[2].starts_with("<Link#") {
            return Some((f[5].parse().ok()?, f[8].parse().ok()?));
        }
    }
    None
}

fn load() -> Store {
    path().map(|p| read_json(&p)).unwrap_or_default()
}

fn save(store: &Store) -> Result<(), String> {
    write_json(&path()?, store)
}

/// Open a session. Any previously-open one is closed first: a crash or a disconnect that happened
/// outside the app would otherwise leave a session open forever and corrupt every total after it.
pub fn begin(region: &str, endpoint: &str) -> Result<(), String> {
    let mut store = load();
    close_open(&mut store);
    store.sessions.push(Session {
        id: now(),
        region: region.to_string(),
        endpoint: endpoint.to_string(),
        started_at: now(),
        ended_at: None,
        rx_bytes: 0,
        tx_bytes: 0,
    });
    // Keeping every session forever would grow without bound for no benefit; a few hundred is
    // more history than anyone looks at.
    let len = store.sessions.len();
    if len > 400 {
        store.sessions.drain(0..len - 400);
    }
    save(&store)
}

fn close_open(store: &mut Store) {
    for s in store.sessions.iter_mut().filter(|s| s.ended_at.is_none()) {
        s.ended_at = Some(now());
    }
}

/// Fold the live counters into the open session. Safe to call repeatedly — it writes absolute
/// readings, not deltas, so a missed poll costs nothing.
pub fn sample() -> Result<Option<Session>, String> {
    let mut store = load();
    let Some(s) = store.sessions.iter_mut().rev().find(|s| s.ended_at.is_none()) else {
        return Ok(None);
    };
    if let Some((rx, tx)) = tunnel_iface().as_deref().and_then(counters) {
        s.rx_bytes = rx;
        s.tx_bytes = tx;
    }
    let snapshot = s.clone();
    save(&store)?;
    Ok(Some(snapshot))
}

/// Take a final reading and close the session. Must run *before* the interface is torn down —
/// once `wg-quick down` has run the counters are gone with it.
pub fn end() -> Result<Option<Session>, String> {
    let _ = sample();
    let mut store = load();
    let Some(s) = store.sessions.iter_mut().rev().find(|s| s.ended_at.is_none()) else {
        return Ok(None);
    };
    s.ended_at = Some(now());
    let snapshot = s.clone();
    save(&store)?;
    Ok(Some(snapshot))
}

pub fn history(limit: usize) -> Vec<Session> {
    let mut v = load().sessions;
    v.reverse();
    v.truncate(limit);
    v
}

#[derive(Serialize, Default)]
pub struct Totals {
    pub sessions: usize,
    pub bytes: u64,
    pub seconds: i64,
}

/// Everything since a cutoff — used for "X of your included traffic this month".
pub fn totals_since(cutoff: i64) -> Totals {
    let store = load();
    let mut t = Totals::default();
    for s in store.sessions.iter().filter(|s| s.started_at >= cutoff) {
        t.sessions += 1;
        t.bytes += s.total();
        t.seconds += s.ended_at.unwrap_or_else(now) - s.started_at;
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_link_row_not_the_address_rows() {
        // Real `netstat -ibn` output. The Link row is 10 columns because it has no Address; the
        // per-address rows are 11 and repeat the same byte counts, so summing them would double.
        let sample = "\
Name       Mtu   Network       Address            Ipkts Ierrs     Ibytes    Opkts Oerrs     Obytes  Coll
utun1      1380  <Link#19>                            0     0        512     7113     0    1407316     0
utun1      1380  fe80::d364: fe80:13::d364:6fd        0     -        512     7113     -    1407316     -";
        let mut found = None;
        for line in sample.lines() {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() >= 9 && f[0] == "utun1" && f[2].starts_with("<Link#") {
                found = Some((f[5].parse::<u64>().unwrap(), f[8].parse::<u64>().unwrap()));
            }
        }
        assert_eq!(found, Some((512, 1_407_316)));
    }

    #[test]
    fn total_adds_both_directions() {
        let s = Session { rx_bytes: 10, tx_bytes: 5, ..Default::default() };
        assert_eq!(s.total(), 15);
    }
}
