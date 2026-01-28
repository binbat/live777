use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::broadcast;

/// 控制消息类型枚举
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum MessageType {
    /// 媒体控制消息（音视频流控制）
    MediaControl,
    /// 云台控制消息（PTZ控制）
    PtzControl,
    /// 通用控制消息（其他控制）
    GeneralControl,
}

impl MessageType {
    /// 获取消息类型的字符串表示
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageType::MediaControl => "media_control",
            MessageType::PtzControl => "ptz_control",
            MessageType::GeneralControl => "general_control",
        }
    }

    /// 从字符串解析消息类型
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "media_control" => Some(MessageType::MediaControl),
            "ptz_control" => Some(MessageType::PtzControl),
            "general_control" => Some(MessageType::GeneralControl),
            _ => None,
        }
    }

    /// 获取消息优先级（数值越小优先级越高）
    pub fn priority(&self) -> u8 {
        match self {
            MessageType::PtzControl => 1,     // 最高优先级
            MessageType::MediaControl => 2,   // 中等优先级
            MessageType::GeneralControl => 3, // 最低优先级
        }
    }
}

/// 统一的控制消息结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlMessage {
    /// 消息类型
    pub msg_type: MessageType,
    /// 目标通道名称
    pub channel: String,
    /// 消息数据
    pub data: serde_json::Value,
    /// 消息优先级（1-255，数值越小优先级越高）
    pub priority: u8,
    /// 时间戳（毫秒）
    pub timestamp: u64,
    /// 消息ID（用于追踪和去重）
    pub message_id: Option<String>,
    /// 发送者ID
    pub sender_id: Option<String>,
}

impl ControlMessage {
    /// 创建新的控制消息
    pub fn new(msg_type: MessageType, channel: String, data: serde_json::Value) -> Self {
        Self {
            priority: msg_type.priority(),
            msg_type,
            channel,
            data,
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
            message_id: None,
            sender_id: None,
        }
    }

    /// 设置消息ID
    pub fn with_message_id(mut self, id: String) -> Self {
        self.message_id = Some(id);
        self
    }

    /// 设置发送者ID
    pub fn with_sender_id(mut self, id: String) -> Self {
        self.sender_id = Some(id);
        self
    }

    /// 将消息序列化为字节数组
    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// 从字节数组反序列化消息
    pub fn from_bytes(data: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(data)
    }

    /// 尝试从原始字节数据解析控制消息
    /// 如果不是JSON格式，则创建一个通用控制消息
    pub fn from_raw_bytes(data: &[u8], default_channel: &str) -> Self {
        // 首先尝试解析为JSON控制消息
        if let Ok(msg) = Self::from_bytes(data) {
            return msg;
        }

        // 如果不是JSON，尝试解析为普通JSON
        if let Ok(json_value) = serde_json::from_slice::<serde_json::Value>(data) {
            // 检查是否包含消息类型信息
            if let Some(msg_type_str) = json_value.get("type").and_then(|v| v.as_str()) {
                let msg_type = MessageType::from_str(msg_type_str)
                    .unwrap_or(MessageType::GeneralControl);
                return Self::new(msg_type, default_channel.to_string(), json_value);
            }
            
            // 检查是否是云台控制消息（包含action字段）
            if json_value.get("action").is_some() {
                return Self::new(MessageType::PtzControl, default_channel.to_string(), json_value);
            }
            
            // 默认为通用控制消息
            return Self::new(MessageType::GeneralControl, default_channel.to_string(), json_value);
        }

        // 如果都不是JSON，创建一个包含原始数据的通用消息
        let raw_data = serde_json::json!({
            "raw_data": String::from_utf8_lossy(data),
            "data_type": "raw"
        });
        Self::new(MessageType::GeneralControl, default_channel.to_string(), raw_data)
    }
}

/// DataChannel组，管理单个类型的消息通道
#[derive(Clone)]
pub struct DataChannelGroup {
    /// 发布端到订阅端的消息通道
    pub publish: broadcast::Sender<Vec<u8>>,
    /// 订阅端到发布端的消息通道
    pub subscribe: broadcast::Sender<Vec<u8>>,
    /// 通道名称
    pub channel_name: String,
    /// 消息类型
    pub msg_type: MessageType,
}

impl DataChannelGroup {
    /// 创建新的DataChannel组
    pub fn new(msg_type: MessageType, channel_name: String) -> Self {
        Self {
            publish: broadcast::channel(1024).0,
            subscribe: broadcast::channel(1024).0,
            channel_name,
            msg_type,
        }
    }

    /// 获取发布端接收器
    pub fn subscribe_publish(&self) -> broadcast::Receiver<Vec<u8>> {
        self.publish.subscribe()
    }

    /// 获取订阅端接收器
    pub fn subscribe_subscribe(&self) -> broadcast::Receiver<Vec<u8>> {
        self.subscribe.subscribe()
    }
}

/// 多DataChannel转发器
#[derive(Clone)]
pub struct MultiDataChannelForward {
    /// 各类型消息的DataChannel组
    pub channels: HashMap<MessageType, DataChannelGroup>,
    /// 默认通道（向后兼容）
    pub default_channel: DataChannelGroup,
}

impl MultiDataChannelForward {
    /// 创建新的多DataChannel转发器
    pub fn new(stream_name: &str) -> Self {
        let mut channels = HashMap::new();
        
        // 创建各类型的DataChannel组
        channels.insert(
            MessageType::MediaControl,
            DataChannelGroup::new(
                MessageType::MediaControl,
                format!("{}_media", stream_name),
            ),
        );
        
        channels.insert(
            MessageType::PtzControl,
            DataChannelGroup::new(
                MessageType::PtzControl,
                format!("{}_ptz", stream_name),
            ),
        );
        
        channels.insert(
            MessageType::GeneralControl,
            DataChannelGroup::new(
                MessageType::GeneralControl,
                format!("{}_general", stream_name),
            ),
        );

        // 默认通道（向后兼容）
        let default_channel = DataChannelGroup::new(
            MessageType::GeneralControl,
            format!("{}_default", stream_name),
        );

        Self {
            channels,
            default_channel,
        }
    }

    /// 获取指定类型的DataChannel组
    pub fn get_channel(&self, msg_type: &MessageType) -> Option<&DataChannelGroup> {
        self.channels.get(msg_type)
    }

    /// 获取默认DataChannel组
    pub fn get_default_channel(&self) -> &DataChannelGroup {
        &self.default_channel
    }

    /// 根据消息类型路由消息到对应的发布通道
    pub fn route_publish_message(&self, data: &[u8]) -> Result<(), broadcast::error::SendError<Vec<u8>>> {
        let control_msg = ControlMessage::from_raw_bytes(data, "default");
        
        if let Some(channel) = self.get_channel(&control_msg.msg_type) {
            channel.publish.send(data.to_vec())?;
        } else {
            // 如果找不到对应通道，使用默认通道
            self.default_channel.publish.send(data.to_vec())?;
        }
        
        Ok(())
    }

    /// 根据消息类型路由消息到对应的订阅通道
    pub fn route_subscribe_message(&self, data: &[u8]) -> Result<(), broadcast::error::SendError<Vec<u8>>> {
        let control_msg = ControlMessage::from_raw_bytes(data, "default");
        
        if let Some(channel) = self.get_channel(&control_msg.msg_type) {
            channel.subscribe.send(data.to_vec())?;
        } else {
            // 如果找不到对应通道，使用默认通道
            self.default_channel.subscribe.send(data.to_vec())?;
        }
        
        Ok(())
    }

    /// 获取所有通道的统计信息
    pub fn get_stats(&self) -> HashMap<String, (usize, usize)> {
        let mut stats = HashMap::new();
        
        for (msg_type, channel) in &self.channels {
            stats.insert(
                msg_type.as_str().to_string(),
                (channel.publish.receiver_count(), channel.subscribe.receiver_count()),
            );
        }
        
        stats.insert(
            "default".to_string(),
            (self.default_channel.publish.receiver_count(), self.default_channel.subscribe.receiver_count()),
        );
        
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_message_type_priority() {
        assert_eq!(MessageType::PtzControl.priority(), 1);
        assert_eq!(MessageType::MediaControl.priority(), 2);
        assert_eq!(MessageType::GeneralControl.priority(), 3);
    }

    #[test]
    fn test_control_message_creation() {
        let data = serde_json::json!({"action": "pan", "direction": "left"});
        let msg = ControlMessage::new(MessageType::PtzControl, "test".to_string(), data);
        
        assert_eq!(msg.msg_type, MessageType::PtzControl);
        assert_eq!(msg.channel, "test");
        assert_eq!(msg.priority, 1);
    }

    #[test]
    fn test_control_message_from_raw_bytes() {
        // 测试JSON云台控制消息
        let ptz_json = r#"{"action": "pan", "direction": "left"}"#;
        let msg = ControlMessage::from_raw_bytes(ptz_json.as_bytes(), "test");
        assert_eq!(msg.msg_type, MessageType::PtzControl);

        // 测试普通JSON消息
        let general_json = r#"{"message": "hello"}"#;
        let msg = ControlMessage::from_raw_bytes(general_json.as_bytes(), "test");
        assert_eq!(msg.msg_type, MessageType::GeneralControl);

        // 测试原始文本
        let raw_text = b"hello world";
        let msg = ControlMessage::from_raw_bytes(raw_text, "test");
        assert_eq!(msg.msg_type, MessageType::GeneralControl);
    }

    #[test]
    fn test_multi_datachannel_forward() {
        let forward = MultiDataChannelForward::new("test_stream");
        
        // 测试通道创建
        assert!(forward.get_channel(&MessageType::PtzControl).is_some());
        assert!(forward.get_channel(&MessageType::MediaControl).is_some());
        assert!(forward.get_channel(&MessageType::GeneralControl).is_some());

        // 测试消息路由
        let ptz_msg = r#"{"action": "pan", "direction": "left"}"#;
        assert!(forward.route_publish_message(ptz_msg.as_bytes()).is_ok());
    }
}