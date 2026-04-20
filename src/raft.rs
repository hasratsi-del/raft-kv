use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::{sleep, Instant};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct VoteRequest { pub term: u64, pub candidate_id: u64 }

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct VoteResponse { pub term: u64, pub granted: bool }

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Heartbeat { pub term: u64, pub leader_id: u64 }

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LogEntry {
    pub term: u64,
    pub index: u64,
    pub key: String,
    pub value: String,
    pub is_delete: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppendEntries {
    pub term: u64,
    pub leader_id: u64,
    pub entry: LogEntry,
    pub leader_commit: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppendResponse {
    pub term: u64,
    pub success: bool,
    pub match_index: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum RaftMessage {
    VoteRequest(VoteRequest),
    VoteResponse(VoteResponse),
    Heartbeat(Heartbeat),
    AppendEntries(AppendEntries),
    AppendResponse(AppendResponse),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Role { Follower, Candidate, Leader }

pub struct RaftState {
    pub id: u64,
    pub role: Role,
    pub current_term: u64,
    pub voted_for: Option<u64>,
    pub last_heartbeat: Instant,
    pub election_timeout_ms: u64,
    pub log: Vec<LogEntry>,
    pub commit_index: u64,
    pub last_applied: u64,
    pub match_index: HashMap<String, u64>,
}

impl RaftState {
    pub fn new(id: u64) -> Self {
        let election_timeout_ms = 3000 + (id * 1000);
        RaftState {
            id,
            role: Role::Follower,
            current_term: 0,
            voted_for: None,
            last_heartbeat: Instant::now(),
            election_timeout_ms,
            log: Vec::new(),
            commit_index: 0,
            last_applied: 0,
            match_index: HashMap::new(),
        }
    }
}

pub struct RaftNode {
    pub state: Arc<Mutex<RaftState>>,
    pub peers: Vec<String>,
    pub port: u16,
    pub store: crate::store::KvStore,
}

impl RaftNode {
    pub fn new(id: u64, port: u16, peers: Vec<String>, store: crate::store::KvStore) -> Self {
        RaftNode {
            state: Arc::new(Mutex::new(RaftState::new(id))),
            peers,
            port,
            store,
        }
    }

    pub async fn run(self: Arc<Self>) {
        let raft_addr = format!("127.0.0.1:{}", self.port);
        let listener = TcpListener::bind(&raft_addr).await.unwrap();
        println!("[Node {}] Raft listening on {}", self.state.lock().unwrap().id, raft_addr);

        let n = self.clone();
        tokio::spawn(async move {
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                let node = n.clone();
                tokio::spawn(async move { node.handle_connection(socket).await; });
            }
        });

        let n = self.clone();
        tokio::spawn(async move { n.run_election_timer().await; });

        let n = self.clone();
        tokio::spawn(async move { n.run_heartbeat_sender().await; });
    }

    pub async fn propose(&self, key: String, value: String, is_delete: bool) -> bool {
        let (is_leader, term, next_index) = {
            let state = self.state.lock().unwrap();
            if state.role != Role::Leader {
                return false;
            }
            let next_index = state.log.len() as u64 + 1;
            (true, state.current_term, next_index)
        };

        if !is_leader { return false; }

        let entry = LogEntry {
            term,
            index: next_index,
            key: key.clone(),
            value: value.clone(),
            is_delete,
        };

        {
            let mut state = self.state.lock().unwrap();
            state.log.push(entry.clone());
            println!("[Node {}] Appended to log: index={} key={} value={}", state.id, next_index, key, value);
        }

        let mut confirmations = 1;
        let majority = (self.peers.len() + 1) / 2 + 1;

        for peer in &self.peers {
            let commit_index = self.state.lock().unwrap().commit_index;
            let leader_id = self.state.lock().unwrap().id;
            let msg = RaftMessage::AppendEntries(AppendEntries {
                term,
                leader_id,
                entry: entry.clone(),
                leader_commit: commit_index,
            });

            if let Some(RaftMessage::AppendResponse(resp)) = send_message(peer, msg).await {
                if resp.success {
                    confirmations += 1;
                    let mut state = self.state.lock().unwrap();
                    state.match_index.insert(peer.clone(), resp.match_index);
                    println!("[Node {}] Follower {} confirmed index {}", state.id, peer, resp.match_index);
                }
            }
        }

        if confirmations >= majority {
            let mut state = self.state.lock().unwrap();
            state.commit_index = next_index;
            state.last_applied = next_index;
            println!("[Node {}] Committed index {} ({}/{})", state.id, next_index, confirmations, majority);
        }

        if is_delete {
            self.store.delete(&key);
        } else {
            self.store.set(&key, &value);
        }

        let node_id = self.state.lock().unwrap().id;
        println!("[Node {}] Applied to store: {} = {}", node_id, key, value);

        confirmations >= majority
    }

    async fn handle_connection(&self, socket: TcpStream) {
        let (reader, mut writer) = socket.into_split();
        let mut lines = BufReader::new(reader).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let msg: RaftMessage = match serde_json::from_str(&line) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if let Some(resp) = self.handle_message(msg) {
                let mut out = serde_json::to_string(&resp).unwrap();
                out.push('\n');
                writer.write_all(out.as_bytes()).await.unwrap();
            }
        }
    }

    fn handle_message(&self, msg: RaftMessage) -> Option<RaftMessage> {
        let mut state = self.state.lock().unwrap();
        match msg {
            RaftMessage::Heartbeat(hb) => {
                if hb.term >= state.current_term {
                    let was_not_follower = state.role != Role::Follower;
                    state.current_term = hb.term;
                    state.role = Role::Follower;
                    state.voted_for = None;
                    state.last_heartbeat = Instant::now();
                    if was_not_follower {
                        println!("[Node {}] Stepped down, following leader {}", state.id, hb.leader_id);
                    }
                }
                None
            }

            RaftMessage::VoteRequest(req) => {
                if req.term > state.current_term {
                    state.current_term = req.term;
                    state.voted_for = None;
                    state.role = Role::Follower;
                }
                let granted = req.term >= state.current_term
                    && (state.voted_for.is_none() || state.voted_for == Some(req.candidate_id));
                if granted {
                    state.voted_for = Some(req.candidate_id);
                }
                println!("[Node {}] Vote for {} (term {}) — {}", state.id, req.candidate_id, req.term, if granted { "GRANTED" } else { "DENIED" });
                Some(RaftMessage::VoteResponse(VoteResponse { term: state.current_term, granted }))
            }

            RaftMessage::AppendEntries(ae) => {
                if ae.term < state.current_term {
                    return Some(RaftMessage::AppendResponse(AppendResponse {
                        term: state.current_term,
                        success: false,
                        match_index: state.log.len() as u64,
                    }));
                }

                state.current_term = ae.term;
                state.role = Role::Follower;
                state.last_heartbeat = Instant::now();
                state.log.push(ae.entry.clone());
                let match_index = state.log.len() as u64;

                if ae.leader_commit > state.commit_index {
                    state.commit_index = ae.leader_commit.min(match_index);
                }

                let key = ae.entry.key.clone();
                let value = ae.entry.value.clone();
                let is_delete = ae.entry.is_delete;
                let node_id = state.id;
                drop(state);

                if is_delete {
                    self.store.delete(&key);
                } else {
                    self.store.set(&key, &value);
                }
                println!("[Node {}] Replicated to store: {} = {}", node_id, key, value);

                let match_index = self.state.lock().unwrap().log.len() as u64;
                Some(RaftMessage::AppendResponse(AppendResponse {
                    term: ae.term,
                    success: true,
                    match_index,
                }))
            }

            RaftMessage::VoteResponse(_) => None,
            RaftMessage::AppendResponse(_) => None,
        }
    }

    async fn run_election_timer(&self) {
        loop {
            sleep(Duration::from_millis(50)).await;
            let (should_start, id) = {
                let state = self.state.lock().unwrap();
                let elapsed = state.last_heartbeat.elapsed().as_millis() as u64;
                let timed_out = elapsed >= state.election_timeout_ms;
                (state.role != Role::Leader && timed_out, state.id)
            };
            if should_start {
                println!("[Node {}] Election timeout — starting election", id);
                { let mut state = self.state.lock().unwrap(); state.last_heartbeat = Instant::now(); }
                self.start_election().await;
            }
        }
    }

    async fn start_election(&self) {
        let (term, id) = {
            let mut state = self.state.lock().unwrap();
            state.role = Role::Candidate;
            state.current_term += 1;
            state.voted_for = Some(state.id);
            println!("[Node {}] Starting election for term {}", state.id, state.current_term);
            (state.current_term, state.id)
        };

        let mut votes = 1;
        let majority = (self.peers.len() + 1) / 2 + 1;

        for peer in &self.peers {
            let req = RaftMessage::VoteRequest(VoteRequest { term, candidate_id: id });
            if let Some(RaftMessage::VoteResponse(resp)) = send_message(peer, req).await {
                if resp.term > term {
                    let mut state = self.state.lock().unwrap();
                    state.current_term = resp.term;
                    state.role = Role::Follower;
                    state.voted_for = None;
                    return;
                }
                if resp.granted {
                    votes += 1;
                    println!("[Node {}] Got vote ({}/{})", id, votes, majority);
                }
            }
        }

        if votes >= majority {
            let mut state = self.state.lock().unwrap();
            if state.role == Role::Candidate && state.current_term == term {
                state.role = Role::Leader;
                println!("\n[Node {}] *** BECAME LEADER for term {} ***\n", state.id, term);
            }
        }
    }

    async fn run_heartbeat_sender(&self) {
        loop {
            sleep(Duration::from_millis(1000)).await;
            let (is_leader, term, id) = {
                let state = self.state.lock().unwrap();
                (state.role == Role::Leader, state.current_term, state.id)
            };
            if is_leader {
                println!("[Node {}] Sending heartbeats for term {}", id, term);
                for peer in &self.peers {
                    let hb = RaftMessage::Heartbeat(Heartbeat { term, leader_id: id });
                    send_heartbeat(peer, hb).await;
                }
            }
        }
    }
}

async fn send_heartbeat(addr: &str, msg: RaftMessage) {
    if let Ok(mut stream) = TcpStream::connect(addr).await {
        let mut out = serde_json::to_string(&msg).unwrap();
        out.push('\n');
        let _ = stream.write_all(out.as_bytes()).await;
        sleep(Duration::from_millis(10)).await;
    }
}

async fn send_message(addr: &str, msg: RaftMessage) -> Option<RaftMessage> {
    let stream = match TcpStream::connect(addr).await {
        Ok(s) => s,
        Err(_) => return None,
    };
    let (reader, mut writer) = stream.into_split();
    let mut out = serde_json::to_string(&msg).unwrap();
    out.push('\n');
    writer.write_all(out.as_bytes()).await.ok()?;
    let mut lines = BufReader::new(reader).lines();
    let line = lines.next_line().await.ok()??;
    serde_json::from_str(&line).ok()
}