# raft-kv

A distributed key-value store built from scratch in Rust, implementing the Raft consensus algorithm for fault-tolerant replication across a 3-node cluster.

## What it does

- Elects a leader automatically using Raft consensus
- Replicates every write to all nodes before confirming
- Survives node crashes — remaining nodes elect a new leader within 5 seconds
- Only commits a write once a majority of nodes confirm it — no data loss
- Single surviving node correctly refuses to become leader without majority

## How to run

Start each node in a separate terminal:

```bash
cargo run -- 1   # KV port 8001, Raft port 9001
cargo run -- 2   # KV port 8002, Raft port 9002
cargo run -- 3   # KV port 8003, Raft port 9003
```

Write to the leader:
```bash
nc 127.0.0.1 8001
{"Set":{"key":"name","value":"alice"}}
```

Read from any follower — data is automatically replicated:
```bash
nc 127.0.0.1 8002
{"Get":{"key":"name"}}
# returns {"Value":"alice"} even though you wrote to 8001
```

## Crash recovery demo

1. Start all 3 nodes
2. Write data to node 1 (leader)
3. Kill node 1 with Ctrl+C
4. Node 2 becomes leader within 5 seconds automatically
5. Write new data to node 2 — cluster keeps working with 2/3 nodes
6. Kill node 2 — node 3 correctly refuses to elect itself (no majority)
7. Restart node 1 — node 3 votes for it, cluster recovers
8. All data preserved throughout

## Architecture


