use stateright::actor::model_timeout;
use stateright::actor::{Actor, ActorModel, Id, Network, Out};
use stateright::report::WriteReporter;
use stateright::Checker;
use stateright::{Expectation, Model};
use std::borrow::Cow;
use std::cmp::min;
use std::collections::HashSet;
use std::hash::Hash;

#[derive(PartialEq, Hash, Eq, Clone, Debug)]
pub enum Role {
    Follower,
    Candidate,
    Leader,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct LogEntry {
    pub(crate) term: usize,
    pub(crate) payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeState {
    pub id: usize,
    pub current_term: usize,
    pub voted_for: Option<usize>,
    pub log: Vec<LogEntry>,
    pub commit_length: usize,
    pub current_role: Role,
    pub current_leader: Option<usize>,
    pub votes_received: HashSet<usize>,
    pub sent_length: Vec<usize>,
    pub acked_length: Vec<usize>,
    pub delivered_messages: Vec<Vec<u8>>,
    pub buffer: Vec<Vec<u8>>,
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
        let mut votes_received: Vec<usize> = self.votes_received.iter().cloned().collect();
        votes_received.sort();
        votes_received.hash(state);
        self.sent_length.hash(state);
        self.acked_length.hash(state);
    }
}

impl NodeState {
    pub fn new(id: usize, peers_len: usize) -> Self {
        Self {
            id,
            current_term: 0,
            voted_for: None,
            log: vec![],
            commit_length: 0,
            current_role: Role::Follower,
            current_leader: None,
            votes_received: HashSet::new(),
            sent_length: vec![0; peers_len],
            acked_length: vec![0; peers_len],
            delivered_messages: vec![],
            buffer: vec![],
        }
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct LogRequestArgs {
    pub leader_id: usize,
    pub term: usize,
    pub prefix_len: usize,
    pub prefix_term: usize,
    pub leader_commit: usize,
    pub suffix: Vec<LogEntry>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct LogResponseArgs {
    pub follower: usize,
    pub term: usize,
    pub ack: usize,
    pub success: bool,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct VoteRequestArgs {
    pub cid: usize,
    pub cterm: usize,
    pub clog_length: usize,
    pub clog_term: usize,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct VoteResponseArgs {
    pub voter_id: usize,
    pub term: usize,
    pub granted: bool,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct BroadcastArgs {
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum RaftMessage {
    VoteRequest(VoteRequestArgs),
    VoteResponse(VoteResponseArgs),
    LogRequest(LogRequestArgs),
    LogResponse(LogResponseArgs),
    Broadcast(Vec<u8>),
}

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum RaftTimer {
    ElectionTimeout,
    ReplicationTimeout,
}

pub struct RaftActor {
    pub peer_ids: Vec<usize>,
}

impl Actor for RaftActor {
    type Msg = RaftMessage;
    type State = NodeState;
    type Timer = RaftTimer;
    type Random = ();
    type Storage = ();

    fn on_start(&self, id: Id, _storage: &Option<Self::Storage>, o: &mut Out<Self>) -> Self::State {
        o.set_timer(RaftTimer::ElectionTimeout, model_timeout());
        o.set_timer(RaftTimer::ReplicationTimeout, model_timeout());
        let id: usize = id.into();
        // broadcast a message (the id of the actor)
        o.send(
            Id::from(id),
            RaftMessage::Broadcast(id.to_string().into_bytes()),
        );
        NodeState::new(id, self.peer_ids.len())
    }

    fn on_msg(
        &self,
        _id: Id,
        state: &mut std::borrow::Cow<Self::State>,
        _src: Id,
        msg: Self::Msg,
        o: &mut Out<Self>,
    ) {
        let state = state.to_mut();
        match msg {
            RaftMessage::VoteRequest(args) => {
                if args.cterm > state.current_term {
                    state.current_term = args.cterm;
                    state.current_role = Role::Follower;
                    state.voted_for = None;
                }

                let mut last_term = 0usize;
                if !state.log.is_empty() {
                    last_term = state.log.last().unwrap().term;
                }

                let log_ok = args.clog_term > last_term
                    || (args.clog_term == last_term && args.clog_length >= state.log.len());

                let mut granted = false;
                if args.cterm == state.current_term
                    && log_ok
                    && (state.voted_for.is_none() || state.voted_for.unwrap() == args.cid)
                {
                    state.voted_for = Some(args.cid);
                    granted = true;
                }

                let msg = VoteResponseArgs {
                    voter_id: state.id,
                    term: state.current_term,
                    granted,
                };
                o.send(Id::from(args.cid), RaftMessage::VoteResponse(msg));
            }
            RaftMessage::VoteResponse(args) => {
                if state.current_role == Role::Candidate
                    && args.term == state.current_term
                    && args.granted
                {
                    state.votes_received.insert(args.voter_id);

                    if state.votes_received.len() >= ((self.peer_ids.len() + 1) + 1) / 2 {
                        state.current_role = Role::Leader;
                        state.current_leader = Some(state.id);
                        self.try_drain_buffer(state, o);

                        for i in 0..self.peer_ids.len() {
                            if i == state.id {
                                continue;
                            }
                            state.sent_length[i] = state.log.len();
                            state.acked_length[i] = 0;
                        }
                        self.handle_replicate_log(state, o);
                    }
                } else if args.term > state.current_term {
                    state.current_term = args.term;
                    state.current_role = Role::Follower;
                    state.voted_for = None;
                    o.set_timer(RaftTimer::ElectionTimeout, model_timeout());
                }
            }
            RaftMessage::LogRequest(args) => {
                if args.term > state.current_term {
                    state.current_term = args.term;
                    state.voted_for = None;
                    o.set_timer(RaftTimer::ElectionTimeout, model_timeout());
                }
                if args.term == state.current_term {
                    state.current_role = Role::Follower;
                    state.current_leader = Some(args.leader_id);
                    self.try_drain_buffer(state, o);
                    o.set_timer(RaftTimer::ElectionTimeout, model_timeout());
                }
                let log_ok = (state.log.len() >= args.prefix_len)
                    && (args.prefix_len == 0
                        || state.log[args.prefix_len - 1].term == args.prefix_term);

                let mut ack = 0;
                let mut success = false;

                if args.term == state.current_term && log_ok {
                    self.append_entries(
                        state,
                        args.prefix_len,
                        args.leader_commit,
                        args.suffix.clone(),
                    );
                    ack = args.prefix_len + args.suffix.len();
                    success = true;
                }
                let msg = LogResponseArgs {
                    follower: state.id,
                    term: state.current_term,
                    ack,
                    success,
                };

                o.send(Id::from(args.leader_id), RaftMessage::LogResponse(msg));
            }
            RaftMessage::LogResponse(args) => {
                if args.term == state.current_term && state.current_role == Role::Leader {
                    if args.success && args.ack >= state.acked_length[args.follower] {
                        state.sent_length[args.follower] = args.ack;
                        state.acked_length[args.follower] = args.ack;
                        self.commit_log_entries(state, self.peer_ids.len());
                    } else if state.sent_length[args.follower] > 0 {
                        state.sent_length[args.follower] -= 1;
                        let id = state.id;
                        self.replicate_log(state, id, args.follower, o);
                    }
                } else if args.term > state.current_term {
                    state.current_term = args.term;
                    state.current_role = Role::Follower;
                    state.voted_for = None;
                    o.set_timer(RaftTimer::ElectionTimeout, model_timeout());
                }
            }
            RaftMessage::Broadcast(payload) => {
                if state.current_role == Role::Leader {
                    let entry = LogEntry {
                        term: state.current_term,
                        payload,
                    };
                    state.log.push(entry.clone());
                    let id = state.id;
                    state.acked_length[id] = state.log.len();
                    self.handle_replicate_log(state, o);
                } else if state.current_leader.is_none() {
                    state.buffer.push(payload);
                } else {
                    o.send(
                        Id::from(state.current_leader.unwrap()),
                        RaftMessage::Broadcast(payload),
                    );
                }
            }
        }
    }

    fn on_timeout(
        &self,
        _id: Id,
        state: &mut Cow<Self::State>,
        timer: &Self::Timer,
        o: &mut Out<Self>,
    ) {
        let state = state.to_mut();
        match timer {
            RaftTimer::ElectionTimeout => {
                if state.current_role == Role::Leader {
                    return;
                }
                let id = state.id;
                state.current_term += 1;
                state.voted_for = Some(id);
                state.current_role = Role::Candidate;
                state.votes_received.clear();
                state.votes_received.insert(id);

                let mut last_term = 0;
                if !state.log.is_empty() {
                    last_term = state.log.last().unwrap().term;
                }

                let msg = VoteRequestArgs {
                    cid: id,
                    cterm: state.current_term,
                    clog_length: state.log.len(),
                    clog_term: last_term,
                };
                for i in 0..self.peer_ids.len() {
                    if i == id {
                        continue;
                    }
                    o.send(Id::from(i), RaftMessage::VoteRequest(msg.clone()));
                }
            }
            RaftTimer::ReplicationTimeout => {
                self.handle_replicate_log(state, o);
            }
        }
    }
}

impl RaftActor {
    fn handle_replicate_log(&self, state: &NodeState, o: &mut Out<Self>) {
        if state.current_role != Role::Leader {
            return;
        }
        let id = state.id;

        for i in 0..self.peer_ids.len() {
            if i == id {
                continue;
            }
            self.replicate_log(state, id, i, o);
        }
    }

    fn replicate_log(
        &self,
        state: &NodeState,
        leader_id: usize,
        follower_id: usize,
        o: &mut Out<Self>,
    ) {
        let prefix_len = state.sent_length[follower_id];
        let suffix = state.log[prefix_len..].to_vec();
        let mut prefix_term = 0;
        if prefix_len > 0 {
            prefix_term = state.log[prefix_len - 1].term;
        }
        let msg = LogRequestArgs {
            leader_id,
            term: state.current_term,
            prefix_len,
            prefix_term,
            leader_commit: state.commit_length,
            suffix,
        };
        o.send(Id::from(follower_id), RaftMessage::LogRequest(msg));
    }

    fn append_entries(
        &self,
        state: &mut NodeState,
        prefix_len: usize,
        leader_commit: usize,
        suffix: Vec<LogEntry>,
    ) {
        if !suffix.is_empty() && state.log.len() > prefix_len {
            let index = min(state.log.len(), prefix_len + suffix.len()) - 1;
            if state.log[index].term != suffix[index - prefix_len].term {
                state.log.truncate(prefix_len);
            }
        }
        if prefix_len + suffix.len() > state.log.len() {
            for suffix_log in suffix.iter().skip(state.log.len() - prefix_len) {
                state.log.push(suffix_log.clone());
            }
        }
        if leader_commit > state.commit_length {
            for i in state.commit_length..leader_commit {
                state.delivered_messages.push(state.log[i].payload.to_vec());
            }
            state.commit_length = leader_commit;
        }
    }

    fn commit_log_entries(&self, state: &mut NodeState, peers_len: usize) {
        let min_acks = ((peers_len + 1) + 1) / 2;
        let mut ready_max = 0;
        for i in state.commit_length + 1..state.log.len() + 1 {
            if Self::acks(&state.acked_length, i) >= min_acks {
                ready_max = i;
            }
        }
        if ready_max > 0 && state.log[ready_max - 1].term == state.current_term {
            for i in state.commit_length..ready_max {
                state.delivered_messages.push(state.log[i].payload.to_vec());
            }
            state.commit_length = ready_max;
        }
    }

    fn acks(acked_length: &Vec<usize>, length: usize) -> usize {
        let mut acks = 0;
        for ack in acked_length {
            if *ack >= length {
                acks += 1;
            }
        }
        acks
    }

    fn try_drain_buffer(&self, state: &mut NodeState, o: &mut Out<Self>) {
        if state.current_role == Role::Leader && !state.buffer.is_empty() {
            for payload in state.buffer.drain(..) {
                o.send(Id::from(state.id), RaftMessage::Broadcast(payload));
            }
        }
    }
}

#[derive(Clone)]
pub struct RaftModelCfg {
    pub server_count: usize,
    pub network: Network<<RaftActor as Actor>::Msg>,
}

impl RaftModelCfg {
    pub fn into_model(self) -> ActorModel<RaftActor, Self> {
        let peers: Vec<usize> = (0..self.server_count).collect();
        ActorModel::new(self.clone(), ())
            .max_crashes((self.server_count - 1) / 2)
            .actors((0..self.server_count).map(|_| RaftActor {
                peer_ids: peers.clone(),
            }))
            .init_network(self.network)
            .property(Expectation::Sometimes, "Election Liveness", |_, state| {
                state
                    .actor_states
                    .iter()
                    .any(|s| s.current_role == Role::Leader)
            })
            .property(Expectation::Sometimes, "Log Liveness", |_, state| {
                state.actor_states.iter().any(|s| s.commit_length > 0)
            })
            .property(Expectation::Always, "Election Safety", |_, state| {
                // at most one leader can be elected in a given term

                let mut leaders_term = HashSet::new();
                for s in &state.actor_states {
                    if s.current_role == Role::Leader && !leaders_term.insert(s.current_term) {
                        return false;
                    }
                }
                true
            })
            .property(Expectation::Always, "State Machine Safety", |_, state| {
                // if a server has applied a log entry at a given index to its state machine, no other server will
                // ever apply a different log entry for the same index.

                let mut max_commit_length = 0;
                let mut max_commit_length_actor_id = 0;
                for (i, s) in state.actor_states.iter().enumerate() {
                    if s.delivered_messages.len() > max_commit_length {
                        max_commit_length = s.delivered_messages.len();
                        max_commit_length_actor_id = i;
                    }
                }
                if max_commit_length == 0 {
                    return true;
                }

                for i in 0..max_commit_length {
                    let ref_log = state.actor_states[max_commit_length_actor_id]
                        .delivered_messages
                        .get(i)
                        .unwrap();
                    for s in &state.actor_states {
                        if let Some(log) = s.delivered_messages.get(i) {
                            if log != ref_log {
                                return false;
                            }
                        }
                    }
                }
                true
            })
    }
}

fn main() -> Result<(), pico_args::Error> {
    env_logger::init_from_env(env_logger::Env::default().default_filter_or("info")); // `RUST_LOG=${LEVEL}` env variable to override

    let mut args = pico_args::Arguments::from_env();
    match args.subcommand()?.as_deref() {
        Some("check") => {
            let server_count = args.opt_free_from_str()?.unwrap_or(3);
            let depth = args.opt_free_from_str()?.unwrap_or(12);
            let network = args
                .opt_free_from_str()?
                .unwrap_or(Network::new_unordered_nonduplicating([]));
            println!("Model checking Raft with {} servers.", server_count);
            RaftModelCfg {
                server_count,
                network,
            }
            .into_model()
            .checker()
            .target_max_depth(depth)
            .threads(num_cpus::get())
            .spawn_bfs()
            .report(&mut WriteReporter::new(&mut std::io::stdout()));
        }
        Some("explore") => {
            let server_count = args.opt_free_from_str()?.unwrap_or(3);
            let address = args
                .opt_free_from_str()?
                .unwrap_or("localhost:3000".to_string());
            let network = args
                .opt_free_from_str()?
                .unwrap_or(Network::new_unordered_nonduplicating([]));
            println!(
                "Exploring state space for Raft with {} servers on {}.",
                server_count, address
            );
            RaftModelCfg {
                server_count,
                network,
            }
            .into_model()
            .checker()
            .threads(num_cpus::get())
            .serve(address);
        }
        _ => {
            println!("USAGE:");
            println!("  ./raft check [SERVER_COUNT] [DEPTH] [NETWORK]");
            println!("  ./raft explore [SERVER_COUNT] [ADDRESS] [NETWORK]");
            println!(
                "NETWORK: {}",
                Network::<<RaftActor as Actor>::Msg>::names().join(" | ")
            );
        }
    }
    Ok(())
}
