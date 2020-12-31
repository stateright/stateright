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
//! - All acknowledged data must eventually be processed. Out of order processing and reprocessing
//!   are both acceptable during a failover.
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
//! - Omits GC of processed data to reduce the state space during checking. The real-world
//!   implemenation must of course prune the session data that it knows have been processed.

#![allow(unused)]

use stateright::*;
use stateright::actor::*;
use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum Msg {
    // External interface with clients.
    Connect,
    ConnAckSuccess,
    Publish(Datum),
    PubAckSuccess,

    // Internal interface between servers.
    Replicate(SessionId, usize, Datum),
    ReplicAck(SessionId, usize),
    Processed(SessionId, usize), // no corresponding ack
    BackfillForClient(Id),
    ProcessSession(SessionId, Session),
}

type Datum = char;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SessionId {
    originator_id: Id, // for uniqueness across primaries
    sequencer: u32,    // for uniqueness within an originator
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct ServerState {
    sessions: BTreeMap<SessionId, Session>,
    open_sessions: BTreeMap<Id, SessionId>, // for appending published messages
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Session {
    client_id: Id,        // for lookup on failover
    role: ServerRole,
    data: Vec<Datum>,
    replica_ids: Vec<Id>, // only one until failover
    replicated_len: usize,
    processed_len: usize, // cannot outpace the replicated len
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum ServerRole {
    Processor,
    Replica,
}

struct Server {
    peer_ids: Vec<Id>,
}

const PROCESSING_INTERVAL: Duration = Duration::from_millis(100);

impl Actor for Server {
    type Msg = Msg;
    type State = ServerState;

    fn on_start(&self, _id: Id, o: &mut Out<Self>) {
        o.set_timer(PROCESSING_INTERVAL..PROCESSING_INTERVAL);
    }

    fn on_msg(&self, _id: Id, _state: &Self::State, _src: Id, msg: Self::Msg, _o: &mut Out<Self>) {
        match msg {
            Msg::Connect => {
                println!("TODO: Close any open sessions for the client, choose a replica, and \
                          start a new open session. Then `ConnAck` and `BackfillForClient`.");
            }
            Msg::Publish(datum) => {
                println!("TODO: `Replicate` to replica. Will `PubAck` upon receiving `ReplicAck`.");
            }
            Msg::Replicate(session_id, index, datum) => {
                println!("TODO: Add to session (or start one), and send `ReplicAck`.");
            }
            Msg::ReplicAck(session_id, index) => {
                println!("TODO: `PubAck` to client.");
            }
            Msg::Processed(session_id, index) => {
                println!("TODO: Revise `processed_len`.");
            }
            Msg::BackfillForClient(client_id) => {
                println!("TODO: Send session states and become replica where necessary.");
            }
            Msg::ProcessSession(session_id, session) => {
                println!("TODO: Merge the incoming session state.");
            }
            _ => {}
        }
    }

    fn on_timeout(&self, _id: Id, _state: &Self::State, _o: &mut Out<Self>) {
        println!("TODO: Process a batch of data.");
    }
}

fn main() {}

#[cfg(test)]
mod test {
    #[test]
    fn test() {}
}
