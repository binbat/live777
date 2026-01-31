use serde::{Deserialize, Serialize};

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
    /// 这个优先级对应UDP端口路由：
    /// - PTZ控制: 端口8890 (最高优先级)
    /// - 媒体控制: 端口8888 (中等优先级)  
    /// - 通用控制: 端口8892 (最低优先级)
    pub fn priority(&self) -> u8 {
        match self {
            MessageType::PtzControl => 1,     // 最高优先级 -> UDP 8890
            MessageType::MediaControl => 2,   // 中等优先级 -> UDP 8888
            MessageType::GeneralControl => 3, // 最低优先级 -> UDP 8892
        }
    }

    /// 获取对应的UDP端口
    pub fn udp_port(&self) -> u16 {
        match self {
            MessageType::PtzControl => 8890,     // 云台控制端口
            MessageType::MediaControl => 8888,   // 媒体控制端口
            MessageType::GeneralControl => 8892, // 通用控制端口
        }
    }
}

/// 统一的控制消息结构
/// 这个结构定义了通过DataChannel传输的消息格式
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlMessage {
    /// 消息类型 - 用于路由到不同UDP端口
    pub message_type: String,
    /// 消息数据
    #[serde(flatten)]
    pub data: serde_json::Value,
    /// 时间戳（毫秒）
    pub timestamp: u64,
}

impl ControlMessage {
    /// 创建新的控制消息
    pub fn new(msg_type: MessageType, data: serde_json::Value) -> Self {
        Self {
            message_type: msg_type.as_str().to_string(),
            data,
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
        }
    }

    /// 从JSON字符串解析控制消息
    pub fn from_json(json_str: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json_str)
    }

    /// 将消息序列化为JSON字符串
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// 获取消息类型枚举
    pub fn get_message_type(&self) -> Option<MessageType> {
        MessageType::from_str(&self.message_type)
    }

    /// 获取目标UDP端口
    pub fn get_target_port(&self) -> u16 {
        self.get_message_type()
            .map(|mt| mt.udp_port())
            .unwrap_or(8888) // 默认端口
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
    fn test_message_type_ports() {
        assert_eq!(MessageType::PtzControl.udp_port(), 8890);
        assert_eq!(MessageType::MediaControl.udp_port(), 8888);
        assert_eq!(MessageType::GeneralControl.udp_port(), 8892);
    }

    #[test]
    fn test_control_message_creation() {
        let data = serde_json::json!({"action": "pan", "direction": "left"});
        let msg = ControlMessage::new(MessageType::PtzControl, data);
        
        assert_eq!(msg.message_type, "ptz_control");
        assert_eq!(msg.get_target_port(), 8890);
    }

    #[test]
    fn test_control_message_parsing() {
        let json_str = r#"{"message_type":"ptz_control","action":"pan","direction":"left","timestamp":1234567890}"#;
        let msg = ControlMessage::from_json(json_str).unwrap();
        
        assert_eq!(msg.message_type, "ptz_control");
        assert_eq!(msg.get_target_port(), 8890);
        assert_eq!(msg.get_message_type(), Some(MessageType::PtzControl));
    }
}