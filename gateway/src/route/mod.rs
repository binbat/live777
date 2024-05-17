use serde::Serialize;
use std::fmt::Debug;

pub mod hook;
pub mod manager;
pub mod proxy;
pub mod r#static;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Page<T: Serialize + Debug> {
    pub page: u64,
    pub page_size: u64,
    pub total: u64,
    pub data: Vec<T>,
}

impl<T: Serialize + Debug> Page<T> {
    pub fn new(page: u64, page_size: u64, total: u64) -> Self {
        Self {
            page,
            page_size,
            total,
            data: vec![],
        }
    }

    pub fn has_next_data(&self) -> bool {
        self.total > (self.page - 1) * self.page_size
    }
}
