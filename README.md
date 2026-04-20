# raft-kv

A distributed key-value store built from scratch in Rust implementing the Raft consensus algorithm. Three independent nodes elect a leader, replicate every write across the cluster, and automatically recover from node failures — with no external dependencies and no consensus libraries.

## Demo

```
# Terminal 1 — Node 1        # Terminal 2 — Node 2        # Terminal 3 — Node 3
cargo run -- 1               cargo run -- 2               cargo run -- 3

[Node 1] BECAME LEADER ★
[Node 1] Sending heartbeats
                             [Node 2] Replicated: name=alice
                                                          [Node 3] Replicated: name=alice
```

## Table of Contents

- [Quick Start](#quick-start)
- [Architecture](#architecture)
- [Traffic Flow Step by Step](#traffic-flow)
- [Core Subsystems](#core-subsystems)
  - [Leader Election](#1-leader-election)
  - [Log Replication](#2-log-replication)
  - [Heartbeats](#3-heartbeats)
  - [Crash Recovery](#4-crash-recovery)
- [Why Raft](#why-raft)
- [Project Structure](#project-structure)
- [Design Decisions](#design-decisions)

## Quick Start

Prerequisites: Rust

```bash
git clone https://github.com/hasratsi-del/raft-kv
cd raft-kv
```

Start all three nodes in separate terminals:

```bash
cargo run -- 1   # KV port 8001, Raft port 9001
cargo run -- 2   # KV port 8002, Raft port 9002
cargo run -- 3   # KV port 8003, Raft port 9003
```

Write to the leader (node 1 wins the first election):

```bash
nc 127.0.0.1 8001
{"Set":{"key":"name","value":"alice"}}
{"Set":{"key":"city","value":"Edmonton"}}
{"Get":{"key":"name"}}
```

Read from a follower — data is automatically replicated:

```bash
nc 127.0.0.1 8002
{"Get":{"key":"name"}}
# {"Value":"alice"} — written to 8001, readable from 8002
```

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        Client                               │
│              nc / your application                          │
└───────────────────────┬─────────────────────────────────────┘
                        │ TCP (JSON)
          ┌─────────────┼─────────────┐
          ▼             ▼             ▼
   ┌────────────┐ ┌────────────┐ ┌────────────┐
   │ KV Server  │ │ KV Server  │ │ KV Server  │
   │  :8001     │ │  :8002     │ │  :8003     │
   └─────┬──────┘ └─────┬──────┘ └─────┬──────┘
         │              │              │
         ▼              ▼              ▼
   ┌────────────┐ ┌────────────┐ ┌────────────┐
   │ Raft Node  │ │ Raft Node  │ │ Raft Node  │
   │  :9001     │ │  :9002     │ │  :9003     │
   │  LEADER ★  │ │  follower  │ │  follower  │
   └─────┬──────┘ └─────┬──────┘ └─────┬──────┘
         │              │              │
         └──── AppendEntries RPC ───────┘
              VoteRequest / Heartbeat
```

Each node runs two servers:
- **KV server** — accepts GET/SET/DELETE from clients
- **Raft server** — handles consensus messages between nodes

A write to any KV server checks if that node is the leader. If yes, it goes through Raft replication. If no, it returns an error telling the client to retry on the leader.

## Traffic Flow

### Frame 1 — Write to Leader

```
Client → SET name alice → Node 1 KV Server (:8001)
                               │
                               ▼
                         Node 1 Raft (:9001)
                         append to log[index=1]
                               │
                    ┌──────────┴──────────┐
                    ▼                     ▼
             Node 2 Raft           Node 3 Raft
             (:9002)                (:9003)
             append to log         append to log
             reply: success        reply: success
                    │                     │
                    └──────────┬──────────┘
                               ▼
                    2/2 followers confirmed
                    majority reached → COMMIT
                    apply to KV store on all nodes
                               │
                               ▼
                    Client ← "Ok"
```

### Frame 2 — Read from Follower

```
Client → GET name → Node 2 KV Server (:8002)
                         │
                         ▼
                   Node 2 KvStore
                   data["name"] = "alice"  ← replicated earlier
                         │
                         ▼
                   Client ← {"Value":"alice"}
```

Reads are served locally from any node. Writes must go through the leader.

### Frame 3 — Leader Crash and Recovery

```
t=0s   Node 1 is leader (term 2), sending heartbeats
t=1s   Node 1 crashes (Ctrl+C)
t=1s   Heartbeats stop arriving
t=5s   Node 2 election timer fires (5s timeout)
       Node 2 → Candidate, term=3
       Node 2 asks Node 3 for vote
       Node 3 → GRANTED
       Node 2 wins majority (2/2 available) → LEADER ★
t=6s   Node 2 sends heartbeats to Node 3
       Cluster operational with 2/3 nodes
t=10s  Node 1 restarts
       Node 1 receives heartbeat from Node 2 (term 3 > term 2)
       Node 1 steps down → follower
       Cluster fully restored
```

### Frame 4 — Majority Safety

```
t=0s   Node 1 leader, Nodes 2+3 followers
t=1s   Kill Node 1
t=5s   Node 2 becomes leader (term 3), Node 3 follower
t=6s   Kill Node 2
t=11s  Node 3 tries to start election
       Node 3 asks... nobody. Only 1 node alive.
       1/2 votes (itself only) — NOT a majority
       Node 3 REFUSES to become leader ✗

       ★ This is correct. A single node cannot guarantee
         it has all committed writes. Becoming leader
         without majority would risk serving stale data.

t=15s  Restart Node 1
       Node 3 votes for Node 1 → majority reached
       Node 1 becomes leader (term 4)
       Cluster recovers ✓
```

## Core Subsystems

### 1. Leader Election

File: `src/raft.rs` — `start_election()`, `run_election_timer()`

Every node has an election timer. If it doesn't hear from a leader within its timeout, it starts an election:

```
Node 1 timeout: 4000ms
Node 2 timeout: 5000ms
Node 3 timeout: 6000ms
```

Different timeouts prevent split votes — Node 1 always fires first on a fresh cluster.

Election process:

```
1. Node 1 timer fires → becomes Candidate
2. Increments term (0 → 1)
3. Votes for itself
4. Sends VoteRequest{term:1, candidate_id:1} to peers
5. Node 2: term 1 > my term 0, haven't voted → GRANT
6. Node 3: term 1 > my term 0, haven't voted → GRANT
7. Node 1 has 3/3 votes (majority=2) → LEADER
8. Sends heartbeats every 1 second to suppress elections
```

Term numbers prevent stale leaders — a node rejects any message from a lower term than its own.

```rust
// A node only grants one vote per term
let granted = req.term >= state.current_term
    && (state.voted_for.is_none()
        || state.voted_for == Some(req.candidate_id));
```

### 2. Log Replication

File: `src/raft.rs` — `propose()`, `handle_message() → AppendEntries`

The log is the source of truth. The KV store is just the result of replaying it.

```
Log:
  index=1  term=1  SET name alice
  index=2  term=1  SET city Edmonton
  index=3  term=2  SET country Canada   ← written after leader change

KV store (derived from log):
  name    = alice
  city    = Edmonton
  country = Canada
```

Write path:

```
1. Client: SET name alice → Leader KV server
2. Leader appends LogEntry{term, index, key, value} to its log
3. Leader sends AppendEntries to all followers
4. Followers append to their logs, reply success
5. Once majority confirm → commit_index advances
6. Entry applied to KV store on all nodes
7. Client receives "Ok"
```

A write is never visible to readers until it is committed. Committed means a majority of nodes have it in their logs — it can never be lost even if the leader crashes immediately after.

### 3. Heartbeats

File: `src/raft.rs` — `run_heartbeat_sender()`, `send_heartbeat()`

The leader sends a `Heartbeat{term, leader_id}` to every follower every 1 second.

When a follower receives a valid heartbeat:

```rust
state.current_term = hb.term;
state.role = Role::Follower;
state.voted_for = None;
state.last_heartbeat = Instant::now(); // reset election timer
```

Resetting `last_heartbeat` is the entire mechanism. As long as heartbeats arrive faster than the election timeout, no follower ever starts an election. If the leader dies, heartbeats stop, timers expire, and election begins.

### 4. Crash Recovery

Nodes are run as independent processes. Kill any one with Ctrl+C.

**Leader crash:**
```
Node 1 (leader) → Ctrl+C
Node 2 timer expires after 5s → starts election
Node 2 wins → new leader
Cluster serves requests with 2/3 nodes
```

**Rejoin:**
```
Node 1 restarts → starts as follower
Receives heartbeat from Node 2 (higher term)
Steps down, updates term, starts following
Data written during outage is NOT on Node 1
(log replication catch-up is the next milestone)
```

**Two nodes dead:**
```
Only Node 3 alive
Node 3 cannot get majority votes → refuses to lead
Cluster correctly unavailable (CAP theorem: CP system)
Restart any second node → election succeeds → recovers
```

## Why Raft

Before Raft (2013), the dominant consensus algorithm was Paxos — correct but notoriously difficult to understand and implement. Raft was designed specifically for understandability, with leader-based replication as the central abstraction.

Real systems built on Raft:
- **etcd** — the database inside every Kubernetes cluster
- **CockroachDB** — distributed SQL
- **TiKV** — the storage layer for TiDB (used at petabyte scale)
- **Consul** — service mesh and configuration

## Project Structure

```
src/
├── main.rs       — node startup, CLI args, KV server
├── raft.rs       — Raft consensus engine
│                   leader election, log replication, heartbeats
├── store.rs      — thread-safe KV store (Arc<Mutex<HashMap>>)
├── protocol.rs   — client message format (JSON over TCP)
└── client.rs     — programmatic TCP client
```

## Message Types

| Message | Direction | Purpose |
|---------|-----------|---------|
| `VoteRequest` | Candidate → Peers | Request vote in election |
| `VoteResponse` | Peer → Candidate | Grant or deny vote |
| `Heartbeat` | Leader → Followers | Suppress elections, prove liveness |
| `AppendEntries` | Leader → Followers | Replicate a log entry |
| `AppendResponse` | Follower → Leader | Confirm log entry received |

## Design Decisions

**Why raw TCP instead of HTTP/gRPC?**

TCP gives full control over framing and connection lifecycle. Each Raft message is a newline-delimited JSON string — simple to debug with netcat, no framework overhead, directly analogous to how real consensus systems communicate.

**Why separate KV ports and Raft ports?**

Clean separation of concerns. Client traffic (8001-8003) never mixes with consensus traffic (9001-9003). Makes it easy to firewall them separately in a real deployment.

**Why fixed election timeouts instead of random?**

For a 3-node local cluster, fixed staggered timeouts (4s/5s/6s) guarantee Node 1 wins the first election with no split votes. In a production system with many nodes and real network variance, random timeouts (150-300ms as in the Raft paper) prevent simultaneous elections more reliably.

**Why is a single surviving node unable to become leader?**

Safety. With 3 nodes, majority = 2. If only 1 node is alive it cannot confirm that no writes were committed by the previous leader to the other nodes. Becoming leader in this state risks serving or overwriting data the client believes was committed. This is the CP (Consistency + Partition tolerance) choice in the CAP theorem — availability is sacrificed to guarantee correctness.

**What's not implemented (yet)**

- Log catch-up for rejoining nodes (node restarts with empty log)
- Log persistence to disk (state lost on restart)
- Dynamic membership changes
- Read linearizability (reads are currently local, not going through Raft)

## Built with

Rust · Tokio (async runtime) · Serde (JSON serialization) · Raw TCP sockets · No consensus libraries