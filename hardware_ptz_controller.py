#!/usr/bin/env python3
"""
真实PTZ云台控制器
监听UDP端口8890，接收PTZ控制命令并转发给真实云台设备
"""

import socket
import json
import time
import threading
from datetime import datetime
from typing import Dict, Any, Optional

class PTZController:
    """PTZ云台控制器基类"""
    
    def __init__(self, device_type: str = "simulator"):
        self.device_type = device_type
        self.current_position = {"pan": 0, "tilt": 0, "zoom": 0}
        self.is_moving = False
        
    def execute_command(self, command: Dict[str, Any]) -> bool:
        """执行PTZ控制命令"""
        action = command.get('action', '')
        direction = command.get('direction', '')
        speed = command.get('speed', 50)
        
        print(f"🎮 [PTZ] 执行命令: {action} {direction} (速度: {speed})")
        
        if action == "pan":
            return self.pan(direction, speed)
        elif action == "tilt":
            return self.tilt(direction, speed)
        elif action == "pan_tilt":
            pan_dir = command.get('pan', '')
            tilt_dir = command.get('tilt', '')
            return self.pan_tilt(pan_dir, tilt_dir, speed)
        elif action == "zoom":
            return self.zoom(direction, speed)
        elif action == "stop":
            return self.stop()
        elif action == "preset":
            preset_id = command.get('preset_id', 1)
            return self.goto_preset(preset_id)
        else:
            print(f"❌ [PTZ] 未知命令: {action}")
            return False
    
    def pan(self, direction: str, speed: int) -> bool:
        """水平转动"""
        if direction == "left":
            print(f"⬅️ [PTZ] 向左转动 (速度: {speed})")
            self.current_position["pan"] -= speed
        elif direction == "right":
            print(f"➡️ [PTZ] 向右转动 (速度: {speed})")
            self.current_position["pan"] += speed
        else:
            print(f"❌ [PTZ] 无效的水平方向: {direction}")
            return False
        
        self.is_moving = True
        return True
    
    def tilt(self, direction: str, speed: int) -> bool:
        """垂直转动"""
        if direction == "up":
            print(f"⬆️ [PTZ] 向上转动 (速度: {speed})")
            self.current_position["tilt"] += speed
        elif direction == "down":
            print(f"⬇️ [PTZ] 向下转动 (速度: {speed})")
            self.current_position["tilt"] -= speed
        else:
            print(f"❌ [PTZ] 无效的垂直方向: {direction}")
            return False
        
        self.is_moving = True
        return True
    
    def pan_tilt(self, pan_dir: str, tilt_dir: str, speed: int) -> bool:
        """同时进行水平和垂直转动"""
        print(f"🔄 [PTZ] 组合转动: 水平{pan_dir} + 垂直{tilt_dir} (速度: {speed})")
        
        success = True
        if pan_dir:
            success &= self.pan(pan_dir, speed)
        if tilt_dir:
            success &= self.tilt(tilt_dir, speed)
        
        return success
    
    def zoom(self, direction: str, speed: int) -> bool:
        """变焦控制"""
        if direction == "in":
            print(f"🔍 [PTZ] 放大 (速度: {speed})")
            self.current_position["zoom"] += speed
        elif direction == "out":
            print(f"🔎 [PTZ] 缩小 (速度: {speed})")
            self.current_position["zoom"] -= speed
        else:
            print(f"❌ [PTZ] 无效的变焦方向: {direction}")
            return False
        
        return True
    
    def stop(self) -> bool:
        """停止所有运动"""
        print(f"⏹️ [PTZ] 停止所有运动")
        self.is_moving = False
        return True
    
    def goto_preset(self, preset_id: int) -> bool:
        """转到预设位置"""
        print(f"📍 [PTZ] 转到预设位置 {preset_id}")
        self.is_moving = True
        # 模拟转到预设位置
        time.sleep(0.1)
        self.is_moving = False
        return True
    
    def get_status(self) -> Dict[str, Any]:
        """获取当前状态"""
        return {
            "position": self.current_position.copy(),
            "is_moving": self.is_moving,
            "device_type": self.device_type,
            "timestamp": time.time()
        }

class SerialPTZController(PTZController):
    """串口PTZ控制器 (Pelco-D协议)"""
    
    def __init__(self, port: str = "COM3", baudrate: int = 9600):
        super().__init__("serial")
        self.port = port
        self.baudrate = baudrate
        self.serial_conn = None
        self.setup_serial()
    
    def setup_serial(self):
        """设置串口连接"""
        try:
            import serial
            self.serial_conn = serial.Serial(
                port=self.port,
                baudrate=self.baudrate,
                timeout=1
            )
            print(f"✅ [PTZ] 串口连接成功: {self.port}")
        except ImportError:
            print("❌ [PTZ] 请安装pyserial: pip install pyserial")
            self.serial_conn = None
        except Exception as e:
            print(f"❌ [PTZ] 串口连接失败: {e}")
            self.serial_conn = None
    
    def send_pelco_command(self, address: int, command1: int, command2: int, data1: int = 0, data2: int = 0):
        """发送Pelco-D协议命令"""
        if not self.serial_conn:
            print("⚠️ [PTZ] 串口未连接，使用模拟模式")
            return True
        
        # Pelco-D协议: 同步字节 + 地址 + 命令1 + 命令2 + 数据1 + 数据2 + 校验和
        sync = 0xFF
        checksum = (address + command1 + command2 + data1 + data2) % 256
        
        packet = bytes([sync, address, command1, command2, data1, data2, checksum])
        
        try:
            self.serial_conn.write(packet)
            print(f"📡 [PTZ] 串口命令已发送: {packet.hex().upper()}")
            return True
        except Exception as e:
            print(f"❌ [PTZ] 串口发送失败: {e}")
            return False
    
    def pan(self, direction: str, speed: int) -> bool:
        """串口水平转动"""
        address = 1  # 设备地址
        
        if direction == "left":
            # 左转: 命令1=0x00, 命令2=0x04, 数据2=速度
            self.send_pelco_command(address, 0x00, 0x04, 0x00, min(speed, 0x3F))
        elif direction == "right":
            # 右转: 命令1=0x00, 命令2=0x02, 数据2=速度
            self.send_pelco_command(address, 0x00, 0x02, 0x00, min(speed, 0x3F))
        
        return super().pan(direction, speed)
    
    def tilt(self, direction: str, speed: int) -> bool:
        """串口垂直转动"""
        address = 1  # 设备地址
        
        if direction == "up":
            # 上转: 命令1=0x00, 命令2=0x08, 数据1=速度
            self.send_pelco_command(address, 0x00, 0x08, min(speed, 0x3F), 0x00)
        elif direction == "down":
            # 下转: 命令1=0x00, 命令2=0x10, 数据1=速度
            self.send_pelco_command(address, 0x00, 0x10, min(speed, 0x3F), 0x00)
        
        return super().tilt(direction, speed)
    
    def stop(self) -> bool:
        """串口停止命令"""
        address = 1  # 设备地址
        # 停止: 命令1=0x00, 命令2=0x00
        self.send_pelco_command(address, 0x00, 0x00, 0x00, 0x00)
        return super().stop()

class HTTPPTZController(PTZController):
    """HTTP PTZ控制器 (海康威视/大华等)"""
    
    def __init__(self, base_url: str, username: str = "admin", password: str = "admin"):
        super().__init__("http")
        self.base_url = base_url.rstrip('/')
        self.auth = (username, password)
        self.session = None
        self.setup_http()
    
    def setup_http(self):
        """设置HTTP会话"""
        try:
            import requests
            self.session = requests.Session()
            self.session.auth = self.auth
            
            # 测试连接
            response = self.session.get(f"{self.base_url}/ISAPI/System/deviceInfo", timeout=5)
            if response.status_code == 200:
                print(f"✅ [PTZ] HTTP连接成功: {self.base_url}")
            else:
                print(f"⚠️ [PTZ] HTTP连接测试失败: {response.status_code}")
        except ImportError:
            print("❌ [PTZ] 请安装requests: pip install requests")
            self.session = None
        except Exception as e:
            print(f"❌ [PTZ] HTTP连接失败: {e}")
            self.session = None
    
    def send_http_command(self, endpoint: str, data: str = None) -> bool:
        """发送HTTP PTZ命令"""
        if not self.session:
            print("⚠️ [PTZ] HTTP未连接，使用模拟模式")
            return True
        
        try:
            url = f"{self.base_url}{endpoint}"
            if data:
                response = self.session.put(url, data=data, timeout=5)
            else:
                response = self.session.get(url, timeout=5)
            
            print(f"📡 [PTZ] HTTP命令响应: {response.status_code}")
            return response.status_code == 200
        except Exception as e:
            print(f"❌ [PTZ] HTTP命令失败: {e}")
            return False
    
    def pan(self, direction: str, speed: int) -> bool:
        """HTTP水平转动"""
        if direction == "left":
            data = f'<PTZData><pan>{-speed}</pan><tilt>0</tilt></PTZData>'
        elif direction == "right":
            data = f'<PTZData><pan>{speed}</pan><tilt>0</tilt></PTZData>'
        else:
            return False
        
        self.send_http_command("/ISAPI/PTZCtrl/channels/1/continuous", data)
        return super().pan(direction, speed)
    
    def tilt(self, direction: str, speed: int) -> bool:
        """HTTP垂直转动"""
        if direction == "up":
            data = f'<PTZData><pan>0</pan><tilt>{speed}</tilt></PTZData>'
        elif direction == "down":
            data = f'<PTZData><pan>0</pan><tilt>{-speed}</tilt></PTZData>'
        else:
            return False
        
        self.send_http_command("/ISAPI/PTZCtrl/channels/1/continuous", data)
        return super().tilt(direction, speed)
    
    def stop(self) -> bool:
        """HTTP停止命令"""
        data = '<PTZData><pan>0</pan><tilt>0</tilt></PTZData>'
        self.send_http_command("/ISAPI/PTZCtrl/channels/1/continuous", data)
        return super().stop()

class PTZUDPListener:
    """PTZ UDP监听器"""
    
    def __init__(self, port: int = 8890, controller: Optional[PTZController] = None):
        self.port = port
        self.controller = controller or PTZController()  # 默认使用模拟器
        self.socket = None
        self.running = False
        
    def start(self):
        """启动UDP监听"""
        try:
            self.socket = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            self.socket.bind(('127.0.0.1', self.port))
            self.socket.settimeout(1.0)
            self.running = True
            
            print(f"🎧 [PTZ] UDP监听器启动，端口: {self.port}")
            print(f"🎮 [PTZ] 控制器类型: {self.controller.device_type}")
            print("=" * 50)
            
            while self.running:
                try:
                    data, addr = self.socket.recvfrom(16384)
                    self.handle_message(data, addr)
                except socket.timeout:
                    continue
                except Exception as e:
                    if self.running:
                        print(f"❌ [PTZ] 接收错误: {e}")
                        
        except Exception as e:
            print(f"❌ [PTZ] 启动失败: {e}")
        finally:
            self.cleanup()
    
    def handle_message(self, data: bytes, addr: tuple):
        """处理接收到的消息"""
        try:
            message_str = data.decode('utf-8')
            message = json.loads(message_str)
            
            timestamp = datetime.now().strftime("%H:%M:%S")
            print(f"\n🎉 [{timestamp}] 收到PTZ控制消息!")
            print(f"📍 来源: {addr}")
            print(f"📄 内容: {message_str}")
            print(f"📏 大小: {len(data)} 字节")
            
            # 检查消息类型
            if message.get('message_type') == 'ptz_control':
                print(f"✅ 消息类型验证通过: PTZ控制")
                
                # 执行PTZ命令
                success = self.controller.execute_command(message)
                
                if success:
                    print(f"✅ PTZ命令执行成功")
                    
                    # 显示当前状态
                    status = self.controller.get_status()
                    print(f"📊 当前位置: Pan={status['position']['pan']}, Tilt={status['position']['tilt']}")
                    print(f"🔄 运动状态: {'运动中' if status['is_moving'] else '静止'}")
                else:
                    print(f"❌ PTZ命令执行失败")
            else:
                print(f"⚠️ 非PTZ控制消息，忽略")
                
        except json.JSONDecodeError:
            print(f"❌ JSON解析失败: {data.decode('utf-8', errors='ignore')}")
        except Exception as e:
            print(f"❌ 消息处理错误: {e}")
    
    def stop(self):
        """停止监听"""
        self.running = False
        print(f"\n🛑 [PTZ] 停止UDP监听器")
    
    def cleanup(self):
        """清理资源"""
        if self.socket:
            self.socket.close()
        print(f"🔌 [PTZ] UDP监听器已关闭")

def main():
    """主函数"""
    print("🚀 PTZ云台控制器启动")
    print("=" * 50)
    
    # 选择控制器类型
    print("请选择PTZ控制器类型:")
    print("1. 模拟器 (默认)")
    print("2. 串口控制器 (Pelco-D)")
    print("3. HTTP控制器 (海康威视/大华)")
    
    choice = input("请输入选择 (1-3): ").strip()
    
    controller = None
    
    if choice == "2":
        port = input("请输入串口端口 (默认COM3): ").strip() or "COM3"
        baudrate = int(input("请输入波特率 (默认9600): ").strip() or "9600")
        controller = SerialPTZController(port, baudrate)
    elif choice == "3":
        base_url = input("请输入设备URL (如 http://192.168.1.100): ").strip()
        username = input("请输入用户名 (默认admin): ").strip() or "admin"
        password = input("请输入密码 (默认admin): ").strip() or "admin"
        if base_url:
            controller = HTTPPTZController(base_url, username, password)
    
    if not controller:
        print("使用模拟PTZ控制器")
        controller = PTZController("simulator")
    
    # 启动UDP监听器
    listener = PTZUDPListener(8890, controller)
    
    try:
        listener.start()
    except KeyboardInterrupt:
        print("\n收到停止信号...")
        listener.stop()

if __name__ == "__main__":
    main()