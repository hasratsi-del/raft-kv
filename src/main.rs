mod store;
mod protocol;
mod client;
mod raft;

use std::sync::Arc;
use store::KvStore;
use protocol::{Request, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

async fn run_kv_server(port: u16, store: KvStore, raft_node: Arc<raft::RaftNode>) {
    let addr = format!("127.0.0.1:{}", port);
    let listener = TcpListener::bind(&addr).await.unwrap();
    println!("[KV {}] Listening", port);

    loop {
        let (socket, _) = listener.accept().await.unwrap();
        let store = store.clone();
        let raft_node = raft_node.clone();

        tokio::spawn(async move {
            let (reader, mut writer) = socket.into_split();
            let mut lines = BufReader::new(reader).lines();

            while let Ok(Some(line)) = lines.next_line().await {
                let request: Request = match serde_json::from_str(&line) {
                    Ok(r) => r,
                    Err(_) => {
                        let err = Response::Error("invalid request".to_string());
                        let mut msg = serde_json::to_string(&err).unwrap();
                        msg.push('\n');
                        writer.write_all(msg.as_bytes()).await.unwrap();
                        continue;
                    }
                };

                let response = match request {
                    Request::Get { key } => Response::Value(store.get(&key)),
                    Request::Set { key, value } => {
                        let success = raft_node.propose(key, value, false).await;
                        if success { Response::Ok }
                        else { Response::Error("not the leader — try another port".to_string()) }
                    }
                    Request::Delete { key } => {
                        let success = raft_node.propose(key, String::new(), true).await;
                        if success { Response::Ok }
                        else { Response::Error("not the leader — try another port".to_string()) }
                    }
                };

                let mut msg = serde_json::to_string(&response).unwrap();
                msg.push('\n');
                writer.write_all(msg.as_bytes()).await.unwrap();
            }
        });
    }
}

#[tokio::main]
async fn main() {
    // Read node ID from command line argument
    // Run as: cargo run -- 1   (for node 1)
    //         cargo run -- 2   (for node 2)
    //         cargo run -- 3   (for node 3)
    let args: Vec<String> = std::env::args().collect();
    let node_id: u64 = args.get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    let (kv_port, raft_port, peers) = match node_id {
        1 => (8001, 9001, vec![
            "127.0.0.1:9002".to_string(),
            "127.0.0.1:9003".to_string(),
        ]),
        2 => (8002, 9002, vec![
            "127.0.0.1:9001".to_string(),
            "127.0.0.1:9003".to_string(),
        ]),
        3 => (8003, 9003, vec![
            "127.0.0.1:9001".to_string(),
            "127.0.0.1:9002".to_string(),
        ]),
        _ => panic!("Node ID must be 1, 2, or 3"),
    };

    println!("Starting node {} (KV port {}, Raft port {})", node_id, kv_port, raft_port);

    let store = KvStore::new();
    let node = Arc::new(raft::RaftNode::new(
        node_id,
        raft_port,
        peers,
        store.clone(),
    ));

    node.clone().run().await;
    tokio::spawn(run_kv_server(kv_port, store, node));

    tokio::signal::ctrl_c().await.unwrap();
    println!("\n[Node {}] Shutting down.", node_id);
}