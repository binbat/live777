#!/usr/bin/env python3
"""
Multi-port UDP listener for testing message routing
Listens on multiple UDP ports simultaneously to verify message routing
"""

import socket
import json
import threading
import time
from datetime import datetime

class MultiPortUDPListener:
    def __init__(self):
        self.ports = {
            8888: "媒体控制",
            8890: "云台控制", 
            8892: "通用控制"
        }
        self.sockets = {}
        self.running = True
        
    def create_socket(self, port):
        """Create and bind UDP socket for a specific port"""
        try:
            sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            sock.bind(('127.0.0.1', port))
            sock.settimeout(1.0)  # 1 second timeout for clean shutdown
            return sock
        except Exception as e:
            print(f"❌ 无法绑定端口 {port}: {e}")
            return None
    
    def listen_on_port(self, port, port_name):
        """Listen for messages on a specific port"""
        sock = self.create_socket(port)
        if not sock:
            return
            
        self.sockets[port] = sock
        print(f"🎧 [{port_name}] 开始监听 UDP 端口 {port}")
        
        while self.running:
            try:
                data, addr = sock.recvfrom(16384)
                timestamp = datetime.now().strftime("%H:%M:%S")
                
                print(f"\n🎉 [{timestamp}] 端口 {port} ({port_name}) 收到消息!")
                print(f"📍 来源: {addr}")
                print(f"📄 内容: {data.decode('utf-8', errors='ignore')}")
                print(f"📏 大小: {len(data)} 字节")
                
                # Try to parse as JSON
                try:
                    json_data = json.loads(data.decode('utf-8'))
                    print(f"📋 JSON解析成功:")
                    
                    # Display message type and routing info
                    if 'message_type' in json_data:
                        msg_type = json_data['message_type']
                        print(f"   🏷️ 消息类型: {msg_type}")
                        
                        # Verify correct routing
                        expected_ports = {
                            'ptz_control': 8890,
                            'media_control': 8888,
                            'general_control': 8892
                        }
                        
                        if msg_type in expected_ports:
                            expected_port = expected_ports[msg_type]
                            if port == expected_port:
                                print(f"   ✅ 路由正确: {msg_type} -> 端口 {port}")
                            else:
                                print(f"   ⚠️ 路由错误: {msg_type} 应该路由到端口 {expected_port}, 但收到在端口 {port}")
                    
                    # Display specific fields based on message type
                    if 'action' in json_data:
                        print(f"   🎮 动作: {json_data['action']}")
                    if 'command' in json_data:
                        print(f"   🎥 命令: {json_data['command']}")
                    if 'timestamp' in json_data:
                        print(f"   🕒 时间戳: {json_data['timestamp']}")
                        
                except json.JSONDecodeError:
                    print(f"📋 非JSON消息")
                
                print(f"✅ 消息处理完成")
                
            except socket.timeout:
                continue
            except Exception as e:
                if self.running:
                    print(f"❌ 端口 {port} 监听错误: {e}")
                break
        
        sock.close()
        print(f"🔌 端口 {port} ({port_name}) 监听已停止")
    
    def start(self):
        """Start listening on all ports"""
        print("🚀 启动多端口UDP监听器")
        print("=" * 50)
        print("端口映射:")
        for port, name in self.ports.items():
            print(f"   端口 {port}: {name}")
        print("=" * 50)
        
        # Start listener threads for each port
        threads = []
        for port, port_name in self.ports.items():
            thread = threading.Thread(
                target=self.listen_on_port, 
                args=(port, port_name),
                daemon=True
            )
            thread.start()
            threads.append(thread)
        
        try:
            print("\n📡 所有端口监听已启动")
            print("💡 提示: 使用 Ctrl+C 停止监听")
            print("🌐 请在浏览器中打开控制界面发送测试消息")
            print("   http://localhost:8080/examples/working_multiport_control.html")
            print("\n等待消息...")
            
            # Keep main thread alive
            while True:
                time.sleep(1)
                
        except KeyboardInterrupt:
            print("\n\n🛑 收到停止信号，正在关闭监听器...")
            self.running = False
            
            # Wait for threads to finish
            for thread in threads:
                thread.join(timeout=2)
            
            print("✅ 多端口UDP监听器已停止")

def main():
    listener = MultiPortUDPListener()
    listener.start()

if __name__ == "__main__":
    main()