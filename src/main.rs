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
                        else { Response::Error("not the leader — try port 8001".to_string()) }
                    }
                    Request::Delete { key } => {
                        let success = raft_node.propose(key, String::new(), true).await;
                        if success { Response::Ok }
                        else { Response::Error("not the leader — try port 8001".to_string()) }
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
    let store1 = KvStore::new();
    let store2 = KvStore::new();
    let store3 = KvStore::new();

    let node1 = Arc::new(raft::RaftNode::new(
        1, 9001,
        vec!["127.0.0.1:9002".to_string(), "127.0.0.1:9003".to_string()],
        store1.clone(),
    ));
    let node2 = Arc::new(raft::RaftNode::new(
        2, 9002,
        vec!["127.0.0.1:9001".to_string(), "127.0.0.1:9003".to_string()],
        store2.clone(),
    ));
    let node3 = Arc::new(raft::RaftNode::new(
        3, 9003,
        vec!["127.0.0.1:9001".to_string(), "127.0.0.1:9002".to_string()],
        store3.clone(),
    ));

    node1.clone().run().await;
    node2.clone().run().await;
    node3.clone().run().await;

    tokio::spawn(run_kv_server(8001, store1, node1));
    tokio::spawn(run_kv_server(8002, store2, node2));
    tokio::spawn(run_kv_server(8003, store3, node3));

    println!("\nCluster ready!");
    println!("Write to port 8001 (will be leader)");
    println!("Read from any port to verify replication\n");

    tokio::signal::ctrl_c().await.unwrap();
    println!("Shutting down.");
}