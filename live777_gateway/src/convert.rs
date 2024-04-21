impl From<crate::config::StorageModel> for live777_storage::StorageModel {
    fn from(value: crate::config::StorageModel) -> Self {
        match value {
            crate::config::StorageModel::RedisStandalone { addr } => {
                live777_storage::StorageModel::RedisStandalone { addr }
            }
        }
    }
}
