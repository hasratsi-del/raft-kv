use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
pub struct KvStore {
    data: Arc<Mutex<HashMap<String, String>>>,
}

impl KvStore {
    pub fn new() -> Self {
        KvStore {
            data: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let data = self.data.lock().unwrap();
        data.get(key).cloned()
    }

    pub fn set(&self, key: &str, value: &str) {
        let mut data = self.data.lock().unwrap();
        data.insert(key.to_string(), value.to_string());
    }

    pub fn delete(&self, key: &str) -> bool {
        let mut data = self.data.lock().unwrap();
        data.remove(key).is_some()
    }
}