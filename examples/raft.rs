use std::collections::HashSet;
use std::hash::Hash;
use std::sync::Arc;
use stateright::actor::model_timeout;
use stateright::actor::{Actor, ActorModel, Id, Network, Out};
use stateright::Expectation;

#[derive(PartialEq, Hash, Eq, Clone, Debug)]
pub enum Role {
    Follower,
    Candidate,
    Leader,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeState {
    pub id: u64,
    pub current_term: u64,
    pub voted_for: Option<u64>,
    pub log: Vec<LogEntry>,
    pub commit_length: u64,
    pub current_role: Role,
    pub current_leader: Option<u64>,
    pub votes_received: HashSet<u64>,
    pub sent_length: Vec<u64>,
    pub acked_length: Vec<u64>,
}

impl Hash for NodeState {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.current_term.hash(state);
        self.voted_for.hash(state);
        self.log.hash(state);
        self.commit_length.hash(state);
        self.current_role.hash(state);
        self.current_leader.hash(state);
        // sort the votes_received to make sure the hash is deterministic
        let mut votes_received: Vec<u64> = self.votes_received.iter().cloned().collect();
        votes_received.sort();
        votes_received.hash(state);
        self.sent_length.hash(state);
        self.acked_length.hash(state);
    }
}

impl NodeState {
    pub fn new(id: u64, peers: Vec<u64>) -> Self {
        Self {
            id,
            current_term: 0,
            voted_for: None,
            log: vec![],
            commit_length: 0,
            current_role: Role::Follower,
            current_leader: None,
            votes_received: HashSet::new(),
            sent_length: vec![0; peers.len()],
            acked_length: vec![0; peers.len()],
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, Hash, PartialEq, Eq)]
pub struct LogRequestArgs {
    pub leader_id: u64,
    pub term: u64,
    pub prefix_len: u64,
    pub prefix_term: u64,
    pub leader_commit: u64,
    pub suffix: Vec<Arc<LogEntry>>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct LogResponseArgs {
    pub follower: u64,
    pub term: u64,
    pub ack: u64,
    pub success: bool,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct VoteRequestArgs {
    pub cid: u64,
    pub cterm: u64,
    pub clog_length: u64,
    pub clog_term: u64,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct VoteResponseArgs {
    pub voter_id: u64,
    pub term: u64,
    pub granted: bool,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct BroadcastArgs {
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum Event {
    ElectionTimeout,
    ReplicationTimeout,
    VoteRequest(VoteRequestArgs),
    VoteResponse(VoteResponseArgs),
    LogRequest(LogRequestArgs),
    LogResponse(LogResponseArgs),
    Broadcast(Vec<u8>),
}

pub struct RaftActor {
    pub state: NodeState,
    pub peer_ids: Vec<u64>,
}

