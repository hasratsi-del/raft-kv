use crate::protocol::{Request, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

pub struct KvClient {
    reader: BufReader<tokio::net::tcp::OwnedReadHalf>,
    writer: tokio::net::tcp::OwnedWriteHalf,
}

impl KvClient {
    pub async fn connect(addr: &str) -> Self {
        let stream = TcpStream::connect(addr).await.unwrap();
        let (reader, writer) = stream.into_split();
        KvClient {
            reader: BufReader::new(reader),
            writer,
        }
    }

    async fn send(&mut self, req: Request) -> Response {
        let mut msg = serde_json::to_string(&req).unwrap();
        msg.push('\n');
        self.writer.write_all(msg.as_bytes()).await.unwrap();
        let mut line = String::new();
        self.reader.read_line(&mut line).await.unwrap();
        serde_json::from_str(line.trim()).unwrap()
    }

    pub async fn set(&mut self, key: &str, value: &str) {
        let req = Request::Set {
            key: key.to_string(),
            value: value.to_string(),
        };
        self.send(req).await;
    }

    pub async fn get(&mut self, key: &str) -> Option<String> {
        let req = Request::Get { key: key.to_string() };
        match self.send(req).await {
            Response::Value(v) => v,
            _ => None,
        }
    }

    pub async fn delete(&mut self, key: &str) -> bool {
        let req = Request::Delete { key: key.to_string() };
        matches!(self.send(req).await, Response::Ok)
    }
}