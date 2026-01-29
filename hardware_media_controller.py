#!/usr/bin/env python3
"""
真实媒体流控制器
监听UDP端口8888，接收媒体控制命令并控制真实的视频流
"""

import socket
import json
import subprocess
import threading
import time
import os
from datetime import datetime
from typing import Dict, Any, Optional, List

class MediaController:
    """媒体流控制器"""
    
    def __init__(self):
        self.ffmpeg_processes = {}  # 存储不同的FFmpeg进程
        self.current_streams = {}   # 当前活动的流
        self.stream_configs = {
            "high": {
                "bitrate": "2000k",
                "fps": "30",
                "resolution": "1920x1080",
                "preset": "ultrafast"
            },
            "medium": {
                "bitrate": "1000k", 
                "fps": "25",
                "resolution": "1280x720",
                "preset": "fast"
            },
            "low": {
                "bitrate": "500k",
                "fps": "15", 
                "resolution": "640x480",
                "preset": "veryfast"
            }
        }
        
    def execute_command(self, command: Dict[str, Any]) -> bool:
        """执行媒体控制命令"""
        cmd = command.get('command', '')
        param = command.get('param', '')
        
        print(f"🎥 [Media] 执行命令: {cmd} {param}")
        
        try:
            if cmd == "start_stream":
                return self.start_stream(param)
            elif cmd == "stop_stream":
                return self.stop_stream(param)
            elif cmd == "set_bitrate":
                return self.set_bitrate(int(param))
            elif cmd == "set_fps":
                return self.set_fps(int(param))
            elif cmd == "set_resolution":
                return self.set_resolution(param)
            elif cmd == "list_streams":
                return self.list_streams()
            elif cmd == "get_status":
                return self.get_status()
            else:
                print(f"❌ [Media] 未知命令: {cmd}")
                return False
        except Exception as e:
            print(f"❌ [Media] 命令执行错误: {e}")
            return False
    
    def start_stream(self, quality: str = "medium") -> bool:
        """启动视频流"""
        if quality in self.current_streams:
            print(f"⚠️ [Media] 流 '{quality}' 已在运行")
            return True
        
        config = self.stream_configs.get(quality, self.stream_configs["medium"])
        
        # 检测可用的视频源
        video_source = self.detect_video_source()
        if not video_source:
            print(f"❌ [Media] 未找到可用的视频源")
            return False
        
        # 构建FFmpeg命令
        cmd = self.build_ffmpeg_command(video_source, config, quality)
        
        try:
            print(f"🚀 [Media] 启动视频流: {quality}")
            print(f"📹 [Media] 视频源: {video_source}")
            print(f"⚙️ [Media] 配置: {config}")
            
            # 启动FFmpeg进程
            process = subprocess.Popen(
                cmd,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                universal_newlines=True
            )
            
            self.ffmpeg_processes[quality] = process
            self.current_streams[quality] = {
                "config": config,
                "source": video_source,
                "start_time": time.time(),
                "process": process
            }
            
            # 启动进程监控线程
            monitor_thread = threading.Thread(
                target=self.monitor_process,
                args=(quality, process),
                daemon=True
            )
            monitor_thread.start()
            
            print(f"✅ [Media] 视频流 '{quality}' 启动成功")
            return True
            
        except Exception as e:
            print(f"❌ [Media] 启动视频流失败: {e}")
            return False
    
    def stop_stream(self, quality: str = "") -> bool:
        """停止视频流"""
        if not quality:
            # 停止所有流
            qualities = list(self.current_streams.keys())
            success = True
            for q in qualities:
                success &= self.stop_stream(q)
            return success
        
        if quality not in self.current_streams:
            print(f"⚠️ [Media] 流 '{quality}' 未在运行")
            return True
        
        try:
            process = self.ffmpeg_processes.get(quality)
            if process:
                print(f"⏹️ [Media] 停止视频流: {quality}")
                process.terminate()
                
                # 等待进程结束
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    print(f"⚠️ [Media] 强制终止进程: {quality}")
                    process.kill()
                
                del self.ffmpeg_processes[quality]
            
            del self.current_streams[quality]
            print(f"✅ [Media] 视频流 '{quality}' 已停止")
            return True
            
        except Exception as e:
            print(f"❌ [Media] 停止视频流失败: {e}")
            return False
    
    def set_bitrate(self, bitrate: int) -> bool:
        """动态调整码率"""
        print(f"📊 [Media] 设置码率: {bitrate}kbps")
        
        # 对于正在运行的流，需要重启以应用新设置
        active_streams = list(self.current_streams.keys())
        
        for quality in active_streams:
            config = self.current_streams[quality]["config"].copy()
            config["bitrate"] = f"{bitrate}k"
            
            # 更新配置
            self.stream_configs[quality] = config
            
            # 重启流以应用新码率
            print(f"🔄 [Media] 重启流 '{quality}' 以应用新码率")
            self.stop_stream(quality)
            time.sleep(1)
            self.start_stream(quality)
        
        return True
    
    def set_fps(self, fps: int) -> bool:
        """动态调整帧率"""
        print(f"🎬 [Media] 设置帧率: {fps}fps")
        
        # 对于正在运行的流，需要重启以应用新设置
        active_streams = list(self.current_streams.keys())
        
        for quality in active_streams:
            config = self.current_streams[quality]["config"].copy()
            config["fps"] = str(fps)
            
            # 更新配置
            self.stream_configs[quality] = config
            
            # 重启流以应用新帧率
            print(f"🔄 [Media] 重启流 '{quality}' 以应用新帧率")
            self.stop_stream(quality)
            time.sleep(1)
            self.start_stream(quality)
        
        return True
    
    def set_resolution(self, resolution: str) -> bool:
        """设置分辨率"""
        print(f"📐 [Media] 设置分辨率: {resolution}")
        
        # 验证分辨率格式
        if 'x' not in resolution:
            print(f"❌ [Media] 无效的分辨率格式: {resolution}")
            return False
        
        # 对于正在运行的流，需要重启以应用新设置
        active_streams = list(self.current_streams.keys())
        
        for quality in active_streams:
            config = self.current_streams[quality]["config"].copy()
            config["resolution"] = resolution
            
            # 更新配置
            self.stream_configs[quality] = config
            
            # 重启流以应用新分辨率
            print(f"🔄 [Media] 重启流 '{quality}' 以应用新分辨率")
            self.stop_stream(quality)
            time.sleep(1)
            self.start_stream(quality)
        
        return True
    
    def list_streams(self) -> bool:
        """列出当前流状态"""
        print(f"📋 [Media] 当前流状态:")
        
        if not self.current_streams:
            print(f"   📭 无活动流")
        else:
            for quality, info in self.current_streams.items():
                runtime = time.time() - info["start_time"]
                print(f"   🎥 {quality}: {info['config']['resolution']} @ {info['config']['fps']}fps, {info['config']['bitrate']} (运行时间: {runtime:.1f}s)")
        
        return True
    
    def get_status(self) -> Dict[str, Any]:
        """获取媒体控制器状态"""
        status = {
            "active_streams": len(self.current_streams),
            "streams": {},
            "timestamp": time.time()
        }
        
        for quality, info in self.current_streams.items():
            status["streams"][quality] = {
                "config": info["config"],
                "source": info["source"],
                "runtime": time.time() - info["start_time"],
                "running": info["process"].poll() is None
            }
        
        return status
    
    def detect_video_source(self) -> Optional[str]:
        """检测可用的视频源"""
        # 优先级顺序：RTSP摄像头 > USB摄像头 > 测试图案
        
        # 1. 尝试RTSP摄像头 (需要用户配置)
        rtsp_urls = [
            "rtsp://admin:admin@192.168.1.100:554/stream1",
            "rtsp://admin:password@192.168.1.100:554/h264/ch1/main/av_stream",
            # 可以添加更多RTSP URL
        ]
        
        for rtsp_url in rtsp_urls:
            if self.test_rtsp_source(rtsp_url):
                return rtsp_url
        
        # 2. 尝试USB摄像头
        usb_sources = self.detect_usb_cameras()
        if usb_sources:
            return usb_sources[0]
        
        # 3. 使用测试图案
        print(f"⚠️ [Media] 未找到真实摄像头，使用测试图案")
        return "testsrc"
    
    def test_rtsp_source(self, rtsp_url: str) -> bool:
        """测试RTSP源是否可用"""
        try:
            # 使用FFprobe快速测试RTSP流
            cmd = [
                "ffprobe",
                "-v", "quiet",
                "-select_streams", "v:0",
                "-show_entries", "stream=width,height",
                "-of", "csv=p=0",
                "-timeout", "5000000",  # 5秒超时
                rtsp_url
            ]
            
            result = subprocess.run(cmd, capture_output=True, timeout=10)
            if result.returncode == 0:
                print(f"✅ [Media] RTSP源可用: {rtsp_url}")
                return True
        except Exception as e:
            pass
        
        return False
    
    def detect_usb_cameras(self) -> List[str]:
        """检测USB摄像头"""
        usb_sources = []
        
        # Windows
        if os.name == 'nt':
            # 尝试检测DirectShow设备
            try:
                cmd = ["ffmpeg", "-list_devices", "true", "-f", "dshow", "-i", "dummy"]
                result = subprocess.run(cmd, capture_output=True, text=True, timeout=10)
                
                # 解析输出查找视频设备
                lines = result.stderr.split('\n')
                for line in lines:
                    if '"' in line and 'video' in line.lower():
                        # 提取设备名称
                        start = line.find('"') + 1
                        end = line.find('"', start)
                        if start > 0 and end > start:
                            device_name = line[start:end]
                            usb_sources.append(f"video={device_name}")
                            print(f"✅ [Media] 发现USB摄像头: {device_name}")
            except Exception:
                pass
        
        # Linux
        else:
            # 检查/dev/video*设备
            for i in range(10):
                device_path = f"/dev/video{i}"
                if os.path.exists(device_path):
                    usb_sources.append(device_path)
                    print(f"✅ [Media] 发现USB摄像头: {device_path}")
        
        return usb_sources
    
    def build_ffmpeg_command(self, video_source: str, config: Dict[str, str], quality: str) -> List[str]:
        """构建FFmpeg命令"""
        cmd = ["ffmpeg"]
        
        # 输入源配置
        if video_source == "testsrc":
            # 测试图案
            cmd.extend([
                "-f", "lavfi",
                "-i", f"testsrc=size={config['resolution']}:rate={config['fps']}"
            ])
        elif video_source.startswith("rtsp://"):
            # RTSP源
            cmd.extend([
                "-rtsp_transport", "tcp",
                "-i", video_source
            ])
        elif video_source.startswith("video="):
            # Windows DirectShow
            cmd.extend([
                "-f", "dshow",
                "-i", video_source
            ])
        elif video_source.startswith("/dev/video"):
            # Linux V4L2
            cmd.extend([
                "-f", "v4l2",
                "-i", video_source
            ])
        
        # 编码配置
        cmd.extend([
            "-c:v", "libx264",
            "-preset", config["preset"],
            "-tune", "zerolatency",
            "-b:v", config["bitrate"],
            "-r", config["fps"],
            "-s", config["resolution"],
            "-pix_fmt", "yuv420p"
        ])
        
        # 输出配置 - 推流到liveion WHIP端点
        stream_name = f"camera_{quality}"
        whip_url = f"http://localhost:7777/whip/{stream_name}"
        
        cmd.extend([
            "-f", "webm",
            "-method", "POST",
            whip_url
        ])
        
        print(f"🔧 [Media] FFmpeg命令: {' '.join(cmd)}")
        return cmd
    
    def monitor_process(self, quality: str, process: subprocess.Popen):
        """监控FFmpeg进程"""
        try:
            stdout, stderr = process.communicate()
            
            if process.returncode != 0:
                print(f"❌ [Media] 流 '{quality}' 异常退出 (代码: {process.returncode})")
                if stderr:
                    print(f"   错误信息: {stderr}")
            else:
                print(f"✅ [Media] 流 '{quality}' 正常退出")
                
        except Exception as e:
            print(f"❌ [Media] 监控进程错误: {e}")
        finally:
            # 清理
            if quality in self.current_streams:
                del self.current_streams[quality]
            if quality in self.ffmpeg_processes:
                del self.ffmpeg_processes[quality]

class MediaUDPListener:
    """媒体控制UDP监听器"""
    
    def __init__(self, port: int = 8888, controller: Optional[MediaController] = None):
        self.port = port
        self.controller = controller or MediaController()
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
            
            print(f"🎧 [Media] UDP监听器启动，端口: {self.port}")
            print("=" * 50)
            
            while self.running:
                try:
                    data, addr = self.socket.recvfrom(16384)
                    self.handle_message(data, addr)
                except socket.timeout:
                    continue
                except Exception as e:
                    if self.running:
                        print(f"❌ [Media] 接收错误: {e}")
                        
        except Exception as e:
            print(f"❌ [Media] 启动失败: {e}")
        finally:
            self.cleanup()
    
    def handle_message(self, data: bytes, addr: tuple):
        """处理接收到的消息"""
        try:
            message_str = data.decode('utf-8')
            message = json.loads(message_str)
            
            timestamp = datetime.now().strftime("%H:%M:%S")
            print(f"\n🎉 [{timestamp}] 收到媒体控制消息!")
            print(f"📍 来源: {addr}")
            print(f"📄 内容: {message_str}")
            print(f"📏 大小: {len(data)} 字节")
            
            # 检查消息类型
            if message.get('message_type') == 'media_control':
                print(f"✅ 消息类型验证通过: 媒体控制")
                
                # 执行媒体命令
                success = self.controller.execute_command(message)
                
                if success:
                    print(f"✅ 媒体命令执行成功")
                    
                    # 显示当前状态
                    status = self.controller.get_status()
                    print(f"📊 活动流数量: {status['active_streams']}")
                    for stream_name, stream_info in status['streams'].items():
                        print(f"   🎥 {stream_name}: {stream_info['config']['resolution']} @ {stream_info['config']['fps']}fps")
                else:
                    print(f"❌ 媒体命令执行失败")
            else:
                print(f"⚠️ 非媒体控制消息，忽略")
                
        except json.JSONDecodeError:
            print(f"❌ JSON解析失败: {data.decode('utf-8', errors='ignore')}")
        except Exception as e:
            print(f"❌ 消息处理错误: {e}")
    
    def stop(self):
        """停止监听"""
        self.running = False
        # 停止所有流
        self.controller.stop_stream()
        print(f"\n🛑 [Media] 停止UDP监听器")
    
    def cleanup(self):
        """清理资源"""
        if self.socket:
            self.socket.close()
        print(f"🔌 [Media] UDP监听器已关闭")

def main():
    """主函数"""
    print("🚀 媒体流控制器启动")
    print("=" * 50)
    
    # 检查FFmpeg是否可用
    try:
        result = subprocess.run(["ffmpeg", "-version"], capture_output=True, timeout=5)
        if result.returncode == 0:
            print("✅ FFmpeg 可用")
        else:
            print("❌ FFmpeg 不可用，请安装FFmpeg")
            return
    except Exception:
        print("❌ FFmpeg 不可用，请安装FFmpeg")
        return
    
    # 创建媒体控制器
    controller = MediaController()
    
    # 启动UDP监听器
    listener = MediaUDPListener(8888, controller)
    
    try:
        listener.start()
    except KeyboardInterrupt:
        print("\n收到停止信号...")
        listener.stop()

if __name__ == "__main__":
    main()