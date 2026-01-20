#!/usr/bin/env python3
"""
测试 liveion UDP 桥接功能的脚本
"""

import socket
import json
import time
import threading
import sys

def udp_listener(port=8888):
    """UDP 监听器，接收来自桥接的消息"""
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    
    # 使用不同的端口来避免与桥接程序冲突
    listen_port = 8889
    sock.bind(('localhost', listen_port))
    print(f"UDP 监听器启动，端口: {listen_port}")
    
    # 先向桥接程序发送注册消息
    try:
        register_msg = {
            "action": "register",
            "client_id": f"listener_{listen_port}",
            "timestamp": int(time.time() * 1000)
        }
        register_data = json.dumps(register_msg).encode('utf-8')
        sock.sendto(register_data, ('localhost', 8888))
        print(f"发送注册消息到桥接程序: {register_msg}")
    except Exception as e:
        print(f"注册失败: {e}")
    
    try:
        while True:
            data, addr = sock.recvfrom(1024)
            message = data.decode('utf-8')
            print(f"[UDP收到] 来自 {addr}: {message}")
            
            # 尝试解析JSON
            try:
                json_data = json.loads(message)
                print(f"[解析] {json_data}")
                
                # 如果是控制指令，发送确认响应
                if json_data.get('action'):
                    response = {
                        "type": "response",
                        "original_action": json_data.get('action'),
                        "status": "received",
                        "timestamp": int(time.time() * 1000)
                    }
                    response_data = json.dumps(response).encode('utf-8')
                    sock.sendto(response_data, addr)
                    print(f"[UDP发送] 响应到 {addr}: {response}")
                    
            except json.JSONDecodeError:
                print(f"[原始] {message}")
                
    except KeyboardInterrupt:
        print("\n停止UDP监听器")
    finally:
        sock.close()

def udp_sender(port=8888):
    """UDP 发送器，发送测试消息"""
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    target = ('localhost', port)
    
    print("UDP 发送器启动，输入消息发送到桥接 (输入 'quit' 退出):")
    
    try:
        while True:
            message = input("发送消息> ")
            if message.lower() == 'quit':
                break
                
            if message.strip():
                # 尝试作为JSON发送
                try:
                    # 如果输入看起来像JSON，直接发送
                    if message.strip().startswith('{'):
                        json.loads(message)  # 验证JSON
                        data = message.encode('utf-8')
                    else:
                        # 否则包装为简单的测试消息
                        test_msg = {
                            "type": "test_message",
                            "content": message,
                            "timestamp": int(time.time() * 1000)
                        }
                        data = json.dumps(test_msg).encode('utf-8')
                except json.JSONDecodeError:
                    # 如果不是有效JSON，作为纯文本发送
                    data = message.encode('utf-8')
                
                sock.sendto(data, target)
                print(f"[已发送] {len(data)} 字节")
                
    except KeyboardInterrupt:
        print("\n停止UDP发送器")
    finally:
        sock.close()

def send_test_commands(port=8888):
    """发送一系列测试命令"""
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    target = ('localhost', port)
    
    test_commands = [
        {"action": "pan", "direction": "left", "speed": 50},
        {"action": "pan", "direction": "right", "speed": 50},
        {"action": "tilt", "direction": "up", "speed": 30},
        {"action": "tilt", "direction": "down", "speed": 30},
        {"action": "zoom", "direction": "in", "value": 1},
        {"action": "zoom", "direction": "out", "value": 1},
        {"action": "stop"},
        {"action": "preset", "number": 1},
        {"action": "custom", "value": 123, "message": "test"}
    ]
    
    print("发送测试命令序列...")
    
    for i, cmd in enumerate(test_commands):
        data = json.dumps(cmd).encode('utf-8')
        sock.sendto(data, target)
        print(f"[{i+1}/{len(test_commands)}] 发送: {cmd}")
        time.sleep(1)
    
    print("测试命令发送完成")
    sock.close()

def main():
    if len(sys.argv) < 2:
        print("用法:")
        print("  python test_liveion_udp.py listen    # 启动UDP监听器")
        print("  python test_liveion_udp.py send      # 启动UDP发送器")
        print("  python test_liveion_udp.py test      # 发送测试命令")
        print("  python test_liveion_udp.py both      # 同时启动监听器和发送器")
        return
    
    mode = sys.argv[1].lower()
    
    if mode == 'listen':
        udp_listener()
    elif mode == 'send':
        udp_sender()
    elif mode == 'test':
        send_test_commands()
    elif mode == 'both':
        # 在后台线程启动监听器
        listener_thread = threading.Thread(target=udp_listener, daemon=True)
        listener_thread.start()
        time.sleep(1)  # 等待监听器启动
        
        # 在主线程运行发送器
        udp_sender()
    else:
        print(f"未知模式: {mode}")

if __name__ == "__main__":
    main()