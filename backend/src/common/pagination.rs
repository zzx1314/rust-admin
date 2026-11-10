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

/// Upper bound applied to [`PageRequest::size`] so that a client cannot ask the
/// database for the whole table with `size=1000000`.
pub const MAX_PAGE_SIZE: i64 = 500;

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
        self.size.clamp(1, MAX_PAGE_SIZE)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_is_clamped_to_the_upper_bound() {
        let request = PageRequest {
            current: 1,
            size: 1_000_000,
        };
        assert_eq!(request.size(), MAX_PAGE_SIZE);
    }

    #[test]
    fn size_is_clamped_to_at_least_one() {
        let request = PageRequest {
            current: 1,
            size: 0,
        };
        assert_eq!(request.size(), 1);

        let negative = PageRequest {
            current: 1,
            size: -20,
        };
        assert_eq!(negative.size(), 1);
    }

    #[test]
    fn size_within_range_is_preserved() {
        let request = PageRequest {
            current: 2,
            size: 25,
        };
        assert_eq!(request.size(), 25);
        assert_eq!(request.offset(), 25);
    }
}
