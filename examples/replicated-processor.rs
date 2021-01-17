//! = Replicated Processor =
//!
//! This is a system in which clients publishes data to a replicated processor node, based on a
//! real-world IOT scenario.
//!
//! == Requirements ==
//!
//! - A client publishes data to a processor, which acknowledges each datum and processes it
//!   asynchronously (as data ingestion may temporarily outpace processing capability in the
//!   real-world scenario).
//! - A client is free to send additional data prior to acknowledgement of a datum published
//!   earlier.
//! - A processor is free to process data before it is replicated, but all acknowledged data must
//!   eventually be processed. Out of order processing and reprocessing are both acceptable during
//!   a failover.
//!
//! == Design Overview ==
//!
//! Steady state:
//!
//! - Upon accepting a client connection, the processor chooses a replica and starts a new session.
//! - Upon receiving a published datum, the processor acks receipt only after the replica acks. If
//!   the replica stops acknowledging, the client will time out and start a new session.
//! - Processing happens in batches and checkpoints to replicas to minimize reprocessing after
//!   failover. Checkpointing is best effort. The real-world implementation would also use
//!   checkpointing for GC, although that is omitted from this model.
//!
//! Failover:
//!
//! - If a client becomes disconnected (e.g. due to timeout), it connects to an arbitrary new 
//!   processor.
//! - The new processor accepts and processes data on the new session. In parallel it
//!   identifies incomplete sessions for the client, backfills the session state, and becomes a
//!   processor for these old sessions as well.
//! - Upon receiving a backfill request, a server becomes a replica for all sessions associated
//!   with that client (i.e. it stops processing if it was doing so). Note that the new processor
//!   does not wait for the old processor to stop, as there is no guarantee that the old
//!   processor will receive the backfill request (e.g. it might be unavailable).
//!
//! == Deviation from the Real-World Implementation ==
//!
//! - Assumes ordered at-most-once delivery for all messages. Will be revised when Stateright
//!   supports modeling TCP semantics, which are more subtle.
//! - Uses the client IP as the client ID, rather than decoupling.
//! - GC is postponed until a session is inactive and fully processed. A bit more bookkeeping would
//!   allow for GC of processed data while a session is active.
//! - Replication requests are not batched in this implementation but could be to reduce load on
//!   the primary.

use serde::{Deserialize, Serialize};
use stateright::actor::{Actor, Id, Out};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::ops::Range;
use std::time::Duration;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
#[derive(Deserialize, Serialize)]
enum Msg {
    // External interface with clients.
    Connect,
    ConnAckSuccess,
    Publish(Datum),
    PubAckSuccess,

    // Internal interface between servers.
    Replicate(SessionId, Id, Vec<Datum>),
    ReplicAck(SessionId, usize),
    Processed(SessionId, usize),
    SyncSession(Id),
    SyncSessAck(SessionId, Id, Vec<Datum>, Vec<Id>, usize),
    CleanUpSession(SessionId),
}

type Datum = char;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[derive(Deserialize, Serialize)]
struct SessionId {
    originator_id: Id, // for uniqueness across primaries
    sequencer: u32,    // for uniqueness within an originator
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ServerState {
    sessions: BTreeMap<SessionId, Session>,
    active_sessions: BTreeMap<Id, SessionId>, // for appending published messages
    next_sequencer: u32,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Session {
    role: ServerRole,
    client_id: Id,        // for lookup on failover
    data: Vec<Datum>,
    replica_ids: Vec<Id>, // [replica1, primary1, primary2, primary3, ...]
    processed_off: usize, // cannot outpace the replicated len
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum ServerRole {
    /// Processors process all their sessions but only publish data to their active sessions.
    Processor,
    /// Replicas are passive. They accept data and they return it on demand. Replicas can be
    /// promoted to processors for a session, but in that case the sessions will be inactive (they
    /// cannot receive additional published data).
    Replica,
}

struct Server {
    peer_ids: Vec<Id>,
}

const PROCESSING_INTERVAL_RANGE: Range<Duration> =
    Duration::from_millis(4_000)..Duration::from_millis(6_000);

impl Actor for Server {
    type Msg = Msg;
    type State = ServerState;

    fn on_start(&self, _id: Id, o: &mut Out<Self>) -> Self::State {
        o.set_timer(PROCESSING_INTERVAL_RANGE);
        ServerState {
            sessions: Default::default(),
            active_sessions: Default::default(),
            next_sequencer: Default::default(),
        }
    }

    fn on_msg(&self, id: Id, state: &mut Cow<Self::State>,
              src: Id, msg: Self::Msg, o: &mut Out<Self>) {
        match msg {
            Msg::Connect => {
                let state = state.to_mut();

                // Start a new active session on an arbitrarily chosen replica, which will receive
                // subsequent data.
                let session_id = SessionId {
                    originator_id: id,
                    sequencer: state.next_sequencer,
                };
                state.sessions.insert(session_id, Session {
                    role: ServerRole::Processor,
                    client_id: src,
                    data: Default::default(),
                    replica_ids: vec![
                        // FIXME: This implementation allows the actor to choose itself as a
                        // replica. The code will be fixed after demonstrating that Stateright
                        // finds this unwanted behavior.
                        self.peer_ids[state.next_sequencer as usize % self.peer_ids.len()],
                        id,
                    ],
                    processed_off: 0,
                });
                state.active_sessions.insert(src, session_id); // overwrites if needed
                state.next_sequencer += 1;

                // Acknowledge and request backfill in case other sessions are incomplete.
                o.send(src, Msg::ConnAckSuccess);
                o.broadcast(&self.peer_ids, &Msg::SyncSession(src));
                // FIXME: If the first replica of a session gets a connection, then it will not
                // realize that it needs to process its own data. The code will be fixed after
                // demonstrating that Stateright finds this unwanted behavior.
            }
            Msg::Publish(datum) => {
                let &session_id = match state.active_sessions.get(&src) {
                    Some(session_id) => session_id,
                    None => return,
                };
                let state = state.to_mut();

                // Persist and forward the datum. Will not `PubAckSuccess` until receiving a
                // `ReplicAck`.
                //
                // FIXME: The real-world implementation performs replication out of band
                // leveraging a larger (non-unit) batch size.
                let session = match state.sessions.get_mut(&session_id) {
                    None => return, // late delivery
                    Some(session) => session,
                };
                assert_eq!(session.client_id, src);
                session.data.push(datum);
                o.send(session.replica_ids[0], Msg::Replicate(session_id, src, vec![datum]));
            }
            Msg::Replicate(session_id, client_id, mut data) => {
                let state = state.to_mut();

                // Create session if necessary, record data, and `ReplicAck`.
                let session = state.sessions.entry(session_id).or_insert(Session {
                    role: ServerRole::Replica,
                    client_id,                  // to facilitate backfill requests
                    data: Default::default(),
                    replica_ids: vec![id, src],
                    processed_off: 0,           // to minimize reprocessing
                });
                assert_eq!(session.client_id, client_id);
                o.send(src, Msg::ReplicAck(session_id, data.len())); // Swapping these two lines
                session.data.append(&mut data);                      // would introduce a bug.
            }
            Msg::ReplicAck(session_id, batch_size) => {
                // Reply to client based on the replicated batch size.
                if let Some(session) = state.sessions.get(&session_id) {
                    // Session may have been garbage collected if another processor is racing this
                    // one.
                    for _ in 0..batch_size {
                        o.send(session.client_id, Msg::PubAckSuccess);
                    }
                }
            }
            Msg::Processed(session_id, processed_off) => {
                let state = state.to_mut();

                // Remove completed sessions, or at least update the processed offset to minimize
                // reprocessing if a new processor backfills from this replica.
                if processed_off == usize::MAX {
                    state.sessions.remove(&session_id);
                } else if let Some(session) = state.sessions.get_mut(&session_id) {
                    // If this node was an earlier processor for the session then it may have
                    // garbage collected it.
                    session.processed_off = processed_off;
                }
            }
            Msg::SyncSession(client_id) => {
                let state = state.to_mut();

                // Send applicable session info, and become a replica. The real-world
                // implementation would have to split the backfill into batches.
                for (&session_id, session) in &mut state.sessions {
                    if session.client_id != client_id { continue }
                    session.role = ServerRole::Replica;
                    state.active_sessions.remove(&session.client_id);
                    o.send(src, Msg::SyncSessAck(
                            session_id,
                            session.client_id,
                            session.data.clone(),
                            session.replica_ids.clone(),
                            session.processed_off.clone()));
                }
            }
            Msg::SyncSessAck(session_id, client_id, data, mut replica_ids, processed_off) => {
                use std::collections::btree_map::Entry::{Occupied, Vacant};
                let state = state.to_mut();

                // Add this session or merge the state (as multiple replicas can send the same
                // sesion with differing data lengths and processed offsets).
                match state.sessions.entry(session_id) {
                    Vacant(entry) => {
                        replica_ids.push(id);
                        entry.insert(Session {
                            role: ServerRole::Replica,
                            client_id,
                            data,
                            replica_ids,
                            processed_off,
                        });
                    }
                    Occupied(mut entry) => {
                        let existing_session = entry.get_mut();
                        if existing_session.data.len() < data.len() {
                            existing_session.data = data;
                        }
                        if existing_session.processed_off < processed_off {
                            existing_session.processed_off = processed_off;
                        }
                        for replica_id in replica_ids {
                            if existing_session.replica_ids.contains(&replica_id) { continue }
                            existing_session.replica_ids.push(replica_id);
                        }
                        existing_session.replica_ids.push(id);
                    }
                }
            }
            _ => {}
        }
    }

    fn on_timeout(&self, _id: Id, state: &mut Cow<Self::State>, o: &mut Out<Self>) {
        let state = state.to_mut();
        o.set_timer(PROCESSING_INTERVAL_RANGE);

        // Process data on each applicable session. GC fully processed inactive sessions.
        // Tell replicas so that they can GC or at least checkpoint to minimize reprocessing if
        // another processor picks up the session.
        let mut active_session_ids: Vec<_> = state.active_sessions.values().cloned().collect();
        active_session_ids.sort();
        let mut removable_session_ids = Vec::new();
        for (&session_id, mut session) in &mut state.sessions {
            if session.role != ServerRole::Processor { continue }
            if session.processed_off == session.data.len() { continue }

            // Processing logic/callback would go here for the real-world implementation. It
            // would also only process a maximum batch size.
            let processed_off = session.data.len();
            session.processed_off = processed_off;
            if active_session_ids.binary_search(&session_id).is_ok() {
                // Active session. Indicate where we are in processing.
                for &replica_id in &session.replica_ids {
                    o.send(replica_id, Msg::Processed(session_id, processed_off));
                }
            } else {
                // Inactive session. Eligible for GC.
                removable_session_ids.push(session_id);
                for &replica_id in &session.replica_ids {
                    o.send(replica_id, Msg::Processed(session_id, usize::MAX));
                }
            }
        }
        for session_id in removable_session_ids {
            state.sessions.remove(&session_id);
        }
    }
}

fn main() {
    use std::net::{SocketAddrV4, Ipv4Addr};
    env_logger::init_from_env(env_logger::Env::default()
        .default_filter_or("info")); // `RUST_LOG=${LEVEL}` env variable to override
    let id0 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 3000));
    let id1 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 3001));
    let id2 = Id::from(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 3002));
    let handles = stateright::actor::spawn(
        serde_json::to_vec,
        |bytes| serde_json::from_slice(bytes),
        vec![
            (id0, Server { peer_ids: vec![id1, id2] }),
            (id1, Server { peer_ids: vec![id0, id2] }),
            (id2, Server { peer_ids: vec![id0, id1] }),
        ]);
    for h in handles { let _ = h.join(); }
}

#[cfg(test)]
mod test {
    use super::*;
    use stateright::{Checker, Model, Property};
    use stateright::actor::{DuplicatingNetwork, Envelope, model_peers, System, SystemModel};

    #[test]
    fn satisfies_properties() {
        TestSystem.into_model().checker()
                .threads(num_cpus::get())
                .spawn_dfs().report(&mut std::io::stdout());

        struct TestSystem;
        impl System for TestSystem {
            type Actor = Server;
            type History = ();
            fn actors(&self) -> Vec<Self::Actor> {
                vec![
                    Server { peer_ids: model_peers(0, 3) },
                    Server { peer_ids: model_peers(1, 3) },
                    Server { peer_ids: model_peers(2, 3) },
                ]
            }
            fn init_network(&self) -> Vec<Envelope<Msg>> {
                vec![
                    // Client pushes some messages to its first processor.
                    Envelope { src: Id::from(4), dst: Id::from(0), msg: Msg::Connect },
                    Envelope { src: Id::from(4), dst: Id::from(0), msg: Msg::Publish('a') },
                    Envelope { src: Id::from(4), dst: Id::from(0), msg: Msg::Publish('b') },
                    // Then client switches to a new processor.
                    Envelope { src: Id::from(4), dst: Id::from(1), msg: Msg::Connect },
                    Envelope { src: Id::from(4), dst: Id::from(1), msg: Msg::Publish('c') },
                ]
            }
            fn duplicating_network(&self) -> DuplicatingNetwork {
                DuplicatingNetwork::No
            }
            fn properties(&self) -> Vec<Property<SystemModel<Self>>> {
                vec![
                    Property::<SystemModel<Self>>::always("true", |_, _state| {
                        true // checker needs at least one property
                    }),
                ]
            }
        }
    }
}
