use std::collections::{HashMap, HashSet};

use crate::tui::{PeerPhase, PeerRow};

use super::{connection::ConnectionHandle, tracker::Peer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    Connecting,
    Handshaking,
    Live,
    Failed,
}

#[derive(Debug)]
pub struct Roster {
    peers: Vec<(String, Lifecycle)>,
    retired: HashSet<String>,
}

impl Roster {
    pub fn from(peers: &[Peer]) -> Self {
        Self {
            peers: peers
                .iter()
                .map(|peer| (peer.address(), Lifecycle::Connecting))
                .collect(),
            retired: HashSet::new(),
        }
    }

    pub fn absorb(&mut self, peers: &[Peer], limit: usize) -> Vec<Peer> {
        let fresh = self
            .unknown(peers)
            .into_iter()
            .take(limit)
            .collect::<Vec<Peer>>();

        self.peers.extend(
            fresh
                .iter()
                .map(|peer| (peer.address(), Lifecycle::Connecting)),
        );

        fresh
    }

    fn unknown(&self, peers: &[Peer]) -> Vec<Peer> {
        peers.iter().fold(Vec::new(), |mut fresh, peer| {
            let address = peer.address();

            let seen = self.knows(&address)
                || fresh.iter().any(|held: &Peer| held.address() == address);

            if !seen {
                fresh.push(peer.clone());
            }

            fresh
        })
    }

    fn knows(&self, address: &str) -> bool {
        self.retired.contains(address) || self.peers.iter().any(|(known, _)| known == address)
    }

    pub fn prune(&mut self, cap: usize) {
        let mut excess = self.peers.len().saturating_sub(cap);

        self.peers.retain(|(address, lifecycle)| {
            let expendable = excess > 0 && *lifecycle == Lifecycle::Failed;

            if expendable {
                excess -= 1;
                self.retired.insert(address.clone());
            }

            !expendable
        });
    }

    pub fn mark(&mut self, address: &str, lifecycle: Lifecycle) {
        if let Some((_, current)) = self.peers.iter_mut().find(|(known, _)| known == address) {
            *current = lifecycle;
        }
    }

    pub fn rows(&self, live: &HashMap<String, ConnectionHandle>) -> Vec<PeerRow> {
        let mut rows = self
            .peers
            .iter()
            .map(|(address, lifecycle)| Self::row(address, *lifecycle, live.get(address)))
            .collect::<Vec<PeerRow>>();

        rows.sort_by_key(|row| row.phase.group());

        rows
    }

    fn row(address: &str, lifecycle: Lifecycle, handle: Option<&ConnectionHandle>) -> PeerRow {
        match (lifecycle, handle) {
            (Lifecycle::Live, Some(handle)) if !handle.tx.is_closed() => PeerRow {
                ip: address.to_string(),
                phase: Self::live_phase(handle),
                available_pieces: handle.available_pieces.len(),
                in_flight: handle.current_piece,
            },
            (Lifecycle::Connecting, _) => Self::pending_row(address, PeerPhase::Connecting),
            (Lifecycle::Handshaking, _) => Self::pending_row(address, PeerPhase::Handshaking),
            _ => Self::pending_row(address, PeerPhase::Failed),
        }
    }

    fn live_phase(handle: &ConnectionHandle) -> PeerPhase {
        match (handle.choked, handle.current_piece) {
            (true, _) => PeerPhase::Choked,
            (false, Some(_)) => PeerPhase::Downloading,
            (false, None) => PeerPhase::Idle,
        }
    }

    fn pending_row(address: &str, phase: PeerPhase) -> PeerRow {
        PeerRow {
            ip: address.to_string(),
            phase,
            available_pieces: 0,
            in_flight: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use super::*;

    fn peers(addresses: &[&str]) -> Vec<Peer> {
        addresses
            .iter()
            .map(|address| {
                let (ip, port) = address.split_once(':').expect("ip:port");

                Peer {
                    ip: ip.to_string(),
                    port: port.parse().expect("port"),
                }
            })
            .collect()
    }

    fn handle(
        address: &str,
        choked: bool,
        current_piece: Option<u32>,
        has: usize,
    ) -> ConnectionHandle {
        let (tx, rx) = mpsc::channel(1);

        Box::leak(Box::new(rx));

        ConnectionHandle {
            address: address.to_string(),
            choked,
            is_downloading: current_piece.is_some(),
            current_piece,
            available_pieces: (0..has as u32).collect(),
            tx,
        }
    }

    fn live(handles: Vec<ConnectionHandle>) -> HashMap<String, ConnectionHandle> {
        handles
            .into_iter()
            .map(|handle| (handle.address.clone(), handle))
            .collect()
    }

    fn phases(rows: &[PeerRow]) -> Vec<PeerPhase> {
        rows.iter().map(|row| row.phase).collect()
    }

    #[test]
    fn every_tracker_peer_is_listed_as_connecting_before_anything_happens() {
        let roster = Roster::from(&peers(&["1.1.1.1:6881", "2.2.2.2:6881", "3.3.3.3:6881"]));

        let rows = roster.rows(&HashMap::new());

        assert_eq!(rows.len(), 3);
        assert_eq!(phases(&rows), vec![PeerPhase::Connecting; 3]);
    }

    #[test]
    fn a_peer_moves_through_connecting_handshaking_and_live() {
        let mut roster = Roster::from(&peers(&["1.1.1.1:6881"]));

        roster.mark("1.1.1.1:6881", Lifecycle::Handshaking);

        assert_eq!(
            phases(&roster.rows(&HashMap::new())),
            vec![PeerPhase::Handshaking]
        );

        roster.mark("1.1.1.1:6881", Lifecycle::Live);

        let connections = live(vec![handle("1.1.1.1:6881", false, None, 42)]);

        assert_eq!(phases(&roster.rows(&connections)), vec![PeerPhase::Idle]);
    }

    #[test]
    fn a_failed_peer_stays_in_the_roster() {
        let mut roster = Roster::from(&peers(&["1.1.1.1:6881", "2.2.2.2:6881"]));

        roster.mark("2.2.2.2:6881", Lifecycle::Failed);

        let rows = roster.rows(&HashMap::new());

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].ip, "2.2.2.2:6881");
        assert_eq!(rows[1].phase, PeerPhase::Failed);
    }

    #[test]
    fn a_live_peer_reports_its_handles_piece_data() {
        let mut roster = Roster::from(&peers(&["1.1.1.1:6881"]));
        roster.mark("1.1.1.1:6881", Lifecycle::Live);

        let rows = roster.rows(&live(vec![handle("1.1.1.1:6881", false, Some(7), 1521)]));

        assert_eq!(rows[0].phase, PeerPhase::Downloading);
        assert_eq!(rows[0].in_flight, Some(7));
        assert_eq!(rows[0].available_pieces, 1521);
    }

    fn order(rows: &[PeerRow]) -> Vec<String> {
        rows.iter().map(|row| row.ip.clone()).collect()
    }

    #[test]
    fn a_choked_peer_still_outranks_one_that_has_not_connected() {
        let mut roster = Roster::from(&peers(&["1.1.1.1:6881", "2.2.2.2:6881", "3.3.3.3:6881"]));

        roster.mark("1.1.1.1:6881", Lifecycle::Handshaking);
        roster.mark("2.2.2.2:6881", Lifecycle::Live);
        roster.mark("3.3.3.3:6881", Lifecycle::Live);

        let connections = live(vec![
            handle("2.2.2.2:6881", true, None, 10),
            handle("3.3.3.3:6881", false, None, 10),
        ]);

        assert_eq!(
            phases(&roster.rows(&connections)),
            vec![PeerPhase::Choked, PeerPhase::Idle, PeerPhase::Handshaking]
        );
    }

    #[test]
    fn a_live_peer_holds_its_row_when_it_stops_downloading() {
        let mut roster = Roster::from(&peers(&["1.1.1.1:6881", "2.2.2.2:6881", "3.3.3.3:6881"]));

        ["1.1.1.1:6881", "2.2.2.2:6881", "3.3.3.3:6881"]
            .iter()
            .for_each(|address| roster.mark(address, Lifecycle::Live));

        let while_downloading = live(vec![
            handle("1.1.1.1:6881", false, None, 10),
            handle("2.2.2.2:6881", false, Some(5), 10),
            handle("3.3.3.3:6881", false, None, 10),
        ]);

        let once_idle = live(vec![
            handle("1.1.1.1:6881", false, None, 10),
            handle("2.2.2.2:6881", false, None, 10),
            handle("3.3.3.3:6881", false, None, 10),
        ]);

        assert_eq!(
            order(&roster.rows(&while_downloading)),
            order(&roster.rows(&once_idle))
        );
    }

    #[test]
    fn a_peer_that_starts_downloading_does_not_jump_the_table() {
        let mut roster = Roster::from(&peers(&["1.1.1.1:6881", "2.2.2.2:6881", "3.3.3.3:6881"]));

        ["1.1.1.1:6881", "2.2.2.2:6881", "3.3.3.3:6881"]
            .iter()
            .for_each(|address| roster.mark(address, Lifecycle::Live));

        let rows = roster.rows(&live(vec![
            handle("1.1.1.1:6881", false, None, 10),
            handle("2.2.2.2:6881", false, None, 10),
            handle("3.3.3.3:6881", false, Some(9), 10),
        ]));

        assert_eq!(order(&rows), vec!["1.1.1.1:6881", "2.2.2.2:6881", "3.3.3.3:6881"]);
    }

    #[test]
    fn working_peers_sort_above_the_dead_tail() {
        let mut roster = Roster::from(&peers(&["1.1.1.1:6881", "2.2.2.2:6881", "3.3.3.3:6881", "4.4.4.4:6881"]));

        roster.mark("1.1.1.1:6881", Lifecycle::Failed);
        roster.mark("3.3.3.3:6881", Lifecycle::Live);

        let rows = roster.rows(&live(vec![handle("3.3.3.3:6881", false, Some(1), 5)]));

        assert_eq!(rows.iter().map(|row| row.ip.as_str()).collect::<Vec<_>>(), vec![
            "3.3.3.3:6881", "2.2.2.2:6881", "4.4.4.4:6881", "1.1.1.1:6881"
        ]);
    }

    #[test]
    fn peers_keep_tracker_order_within_a_phase() {
        let roster = Roster::from(&peers(&["9.9.9.9:6881", "1.1.1.1:6881", "5.5.5.5:6881"]));

        let rows = roster.rows(&HashMap::new());

        assert_eq!(rows.iter().map(|row| row.ip.as_str()).collect::<Vec<_>>(), vec![
            "9.9.9.9:6881", "1.1.1.1:6881", "5.5.5.5:6881"
        ]);
    }

    #[test]
    fn a_live_peer_whose_connection_dropped_is_reported_as_failed() {
        let mut roster = Roster::from(&peers(&["1.1.1.1:6881"]));
        roster.mark("1.1.1.1:6881", Lifecycle::Live);

        let (tx, rx) = mpsc::channel(1);
        drop(rx);

        let connections = live(vec![ConnectionHandle {
            address: "1.1.1.1:6881".to_string(),
            choked: false,
            is_downloading: false,
            current_piece: None,
            available_pieces: HashSet::new(),
            tx,
        }]);

        assert_eq!(phases(&roster.rows(&connections)), vec![PeerPhase::Failed]);
    }

    #[test]
    fn two_peers_on_one_ip_are_tracked_independently() {
        let mut roster = Roster::from(&peers(&["1.1.1.1:6881", "1.1.1.1:51413"]));

        roster.mark("1.1.1.1:51413", Lifecycle::Live);

        let rows = roster.rows(&live(vec![handle("1.1.1.1:51413", false, Some(3), 8)]));

        assert_eq!(
            rows.iter()
                .map(|row| (row.ip.as_str(), row.phase))
                .collect::<Vec<_>>(),
            vec![
                ("1.1.1.1:51413", PeerPhase::Downloading),
                ("1.1.1.1:6881", PeerPhase::Connecting),
            ]
        );
    }

    #[test]
    fn absorbs_only_addresses_it_does_not_already_know() {
        let mut roster = Roster::from(&peers(&["1.1.1.1:6881", "2.2.2.2:6881"]));

        let fresh = roster.absorb(&peers(&["2.2.2.2:6881", "3.3.3.3:6881"]), 10);

        assert_eq!(
            fresh.iter().map(Peer::address).collect::<Vec<_>>(),
            vec!["3.3.3.3:6881"]
        );
        assert_eq!(roster.rows(&HashMap::new()).len(), 3);
    }

    #[test]
    fn the_same_address_twice_in_one_batch_is_absorbed_once() {
        let mut roster = Roster::from(&peers(&["1.1.1.1:6881"]));

        let fresh = roster.absorb(&peers(&["9.9.9.9:6881", "9.9.9.9:6881"]), 10);

        assert_eq!(fresh.len(), 1);
        assert_eq!(roster.rows(&HashMap::new()).len(), 2);
    }

    #[test]
    fn absorbing_stops_at_the_limit() {
        let mut roster = Roster::from(&peers(&["1.1.1.1:6881"]));

        let fresh = roster.absorb(&peers(&["2.2.2.2:6881", "3.3.3.3:6881", "4.4.4.4:6881"]), 2);

        assert_eq!(
            fresh.iter().map(Peer::address).collect::<Vec<_>>(),
            vec!["2.2.2.2:6881", "3.3.3.3:6881"]
        );
        assert_eq!(roster.rows(&HashMap::new()).len(), 3);
    }

    #[test]
    fn a_limit_of_zero_absorbs_nothing() {
        let mut roster = Roster::from(&peers(&["1.1.1.1:6881"]));

        assert!(roster.absorb(&peers(&["2.2.2.2:6881"]), 0).is_empty());
        assert_eq!(roster.rows(&HashMap::new()).len(), 1);
    }

    #[test]
    fn an_absorbed_peer_starts_out_connecting_and_keeps_the_existing_order() {
        let mut roster = Roster::from(&peers(&["1.1.1.1:6881"]));
        roster.mark("1.1.1.1:6881", Lifecycle::Failed);

        roster.absorb(&peers(&["2.2.2.2:6881"]), 10);

        let rows = roster.rows(&HashMap::new());

        assert_eq!(
            rows.iter()
                .map(|row| (row.ip.as_str(), row.phase))
                .collect::<Vec<_>>(),
            vec![
                ("2.2.2.2:6881", PeerPhase::Connecting),
                ("1.1.1.1:6881", PeerPhase::Failed),
            ]
        );
    }

    #[test]
    fn absorbing_a_peer_that_already_failed_does_not_resurrect_it() {
        let mut roster = Roster::from(&peers(&["1.1.1.1:6881"]));
        roster.mark("1.1.1.1:6881", Lifecycle::Failed);

        assert!(roster.absorb(&peers(&["1.1.1.1:6881"]), 10).is_empty());
        assert_eq!(
            phases(&roster.rows(&HashMap::new())),
            vec![PeerPhase::Failed]
        );
    }

    #[test]
    fn pruning_drops_the_oldest_failed_rows_once_the_cap_is_passed() {
        let mut roster = Roster::from(&peers(&[
            "1.1.1.1:6881",
            "2.2.2.2:6881",
            "3.3.3.3:6881",
            "4.4.4.4:6881",
        ]));

        ["1.1.1.1:6881", "2.2.2.2:6881", "3.3.3.3:6881"]
            .iter()
            .for_each(|address| roster.mark(address, Lifecycle::Failed));

        roster.prune(2);

        assert_eq!(
            roster
                .rows(&HashMap::new())
                .iter()
                .map(|row| row.ip.clone())
                .collect::<Vec<_>>(),
            vec!["4.4.4.4:6881", "3.3.3.3:6881"]
        );
    }

    #[test]
    fn pruning_sacrifices_only_failures_even_when_that_misses_the_cap() {
        let mut roster = Roster::from(&peers(&["1.1.1.1:6881", "2.2.2.2:6881", "3.3.3.3:6881"]));

        roster.mark("1.1.1.1:6881", Lifecycle::Live);
        roster.mark("2.2.2.2:6881", Lifecycle::Handshaking);
        roster.mark("3.3.3.3:6881", Lifecycle::Failed);

        roster.prune(1);

        let mut remaining = roster
            .rows(&HashMap::new())
            .iter()
            .map(|row| row.ip.clone())
            .collect::<Vec<_>>();
        remaining.sort();

        assert_eq!(remaining, vec!["1.1.1.1:6881", "2.2.2.2:6881"]);
    }

    #[test]
    fn a_pruned_address_is_still_known_and_is_never_dialled_again() {
        let mut roster = Roster::from(&peers(&["1.1.1.1:6881", "2.2.2.2:6881"]));

        roster.mark("1.1.1.1:6881", Lifecycle::Failed);
        roster.prune(1);

        assert_eq!(roster.rows(&HashMap::new()).len(), 1);
        assert!(roster.absorb(&peers(&["1.1.1.1:6881"]), 10).is_empty());
        assert_eq!(roster.rows(&HashMap::new()).len(), 1);
    }

    #[test]
    fn pruning_below_the_cap_changes_nothing() {
        let mut roster = Roster::from(&peers(&["1.1.1.1:6881", "2.2.2.2:6881"]));
        roster.mark("1.1.1.1:6881", Lifecycle::Failed);

        roster.prune(10);

        assert_eq!(roster.rows(&HashMap::new()).len(), 2);
    }

    #[test]
    fn marking_an_unknown_peer_is_ignored() {
        let mut roster = Roster::from(&peers(&["1.1.1.1:6881"]));

        roster.mark("9.9.9.9:6881", Lifecycle::Live);

        assert_eq!(
            phases(&roster.rows(&HashMap::new())),
            vec![PeerPhase::Connecting]
        );
    }
}
