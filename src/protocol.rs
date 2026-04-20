use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub enum Request {
    Get { key: String },
    Set { key: String, value: String },
    Delete { key: String },
}

#[derive(Serialize, Deserialize, Debug)]
pub enum Response {
    Value(Option<String>),
    Ok,
    Error(String),
}