use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Clone)]
pub struct PageRequest {
    #[serde(default = "default_current")]
    pub current: i64,
    #[serde(default = "default_size")]
    pub size: i64,
}

fn default_current() -> i64 {
    1
}

fn default_size() -> i64 {
    10
}

impl Default for PageRequest {
    fn default() -> Self {
        Self {
            current: 1,
            size: 10,
        }
    }
}

impl PageRequest {
    pub fn page(&self) -> i64 {
        self.current.max(1)
    }

    pub fn size(&self) -> i64 {
        self.size.max(1)
    }

    pub fn offset(&self) -> i64 {
        (self.page() - 1) * self.size()
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PageResponse<T> {
    pub records: Vec<T>,
    pub total: i64,
    pub current: i64,
    pub size: i64,
}

impl<T> PageResponse<T> {
    pub fn new(records: Vec<T>, total: i64, current: i64, size: i64) -> Self {
        Self {
            records,
            total,
            current,
            size,
        }
    }
}
