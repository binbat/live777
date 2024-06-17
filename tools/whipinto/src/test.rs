use tokio::sync::Mutex;
use std::sync::Arc;

#[derive(Debug, Clone)]
struct RTCSessionDescription {
    sdp: String,
}

struct Internal {
    current_local_description: Mutex<Option<RTCSessionDescription>>,
}

struct RTCPeerConnection {
    internal: Arc<Internal>,
}

impl RTCPeerConnection {
    pub async fn modify_local_description(&self) -> Result<(), String> {
        // 锁定并获取引用
        let mut current_local_description = self.internal.current_local_description.lock().await;
        
        // 修改 current_local_description 的内部数据
        if current_local_description.is_none() {
            *current_local_description = Some(RTCSessionDescription {
                sdp: "new_sdp".to_string(),
            });
        } else if let Some(ref mut desc) = *current_local_description {
            desc.sdp = "modified_sdp".to_string();
        }

        // 打印修改后的 current_local_description
        println!("Modified: {:?}", current_local_description);

        Ok(())
    }

    pub async fn print_local_description(&self) {
        let current_local_description = self.internal.current_local_description.lock().await;
        println!("Current: {:?}", *current_local_description);
    }
}

#[tokio::main]
async fn main() {
    // 创建 RTCPeerConnection 的实例并初始化
    let internal = Arc::new(Internal {
        current_local_description: Mutex::new(None),
    });

    let peer_connection = RTCPeerConnection { internal };

    // 打印初始状态
    peer_connection.print_local_description().await;

    // 修改并打印修改后的状态
    peer_connection.modify_local_description().await.unwrap();
    peer_connection.print_local_description().await;

    // 再次修改并打印修改后的状态
    peer_connection.modify_local_description().await.unwrap();
    peer_connection.print_local_description().await;
}

