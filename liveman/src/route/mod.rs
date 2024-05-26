use serde::Serialize;
use std::fmt::Debug;

pub mod embed;
pub mod hook;
pub mod manager;
pub mod proxy;
pub mod r#static;

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Page<T: Serialize + Debug> {
    pub page_no: u64,
    pub page_size: u64,
    pub total: u64,
    pub page: u64,
    pub data: Vec<T>,
}

impl<T: Serialize + Debug> Page<T> {
    pub fn new(page_no: u64, page_size: u64, total: u64) -> Self {
        Self {
            page_no,
            page_size,
            total,
            page: total.div_ceil(page_size),
            data: vec![],
        }
    }

    pub fn has_next_data(&self) -> bool {
        self.total > (self.page_no - 1) * self.page_size
    }
}
