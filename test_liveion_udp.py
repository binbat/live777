#!/usr/bin/env python3
"""
Test script for liveion UDP bridge functionality
"""

import socket
import json
import time
import threading
import sys

def udp_listener(port=8888):
    """UDP listener to receive messages from bridge"""
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    
    # Use different port to avoid conflicts with bridge program
    listen_port = 8889
    sock.bind(('localhost', listen_port))
    print(f"UDP listener started on port: {listen_port}")
    
    # Send registration message to bridge program first
    try:
        register_msg = {
            "action": "register",
            "client_id": f"listener_{listen_port}",
            "timestamp": int(time.time() * 1000)
        }
        register_data = json.dumps(register_msg).encode('utf-8')
        sock.sendto(register_data, ('localhost', 8888))
        print(f"Sent registration message to bridge: {register_msg}")
    except Exception as e:
        print(f"Registration failed: {e}")
    
    try:
        while True:
            data, addr = sock.recvfrom(1024)
            message = data.decode('utf-8')
            print(f"[UDP Received] from {addr}: {message}")
            
            # Try to parse JSON
            try:
                json_data = json.loads(message)
                print(f"[Parsed] {json_data}")
                
                # If it's a control command, send confirmation response
                if json_data.get('action'):
                    response = {
                        "type": "response",
                        "original_action": json_data.get('action'),
                        "status": "received",
                        "timestamp": int(time.time() * 1000)
                    }
                    response_data = json.dumps(response).encode('utf-8')
                    sock.sendto(response_data, addr)
                    print(f"[UDP Sent] response to {addr}: {response}")
                    
            except json.JSONDecodeError:
                print(f"[Raw] {message}")
                
    except KeyboardInterrupt:
        print("\nStopping UDP listener")
    finally:
        sock.close()

def udp_sender(port=8888):
    """UDP sender to send test messages"""
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    target = ('localhost', port)
    
    print("UDP sender started, enter messages to send to bridge (enter 'quit' to exit):")
    
    try:
        while True:
            message = input("Send message> ")
            if message.lower() == 'quit':
                break
                
            if message.strip():
                # Try to send as JSON
                try:
                    # If input looks like JSON, send directly
                    if message.strip().startswith('{'):
                        json.loads(message)  # Validate JSON
                        data = message.encode('utf-8')
                    else:
                        # Otherwise wrap as simple test message
                        test_msg = {
                            "type": "test_message",
                            "content": message,
                            "timestamp": int(time.time() * 1000)
                        }
                        data = json.dumps(test_msg).encode('utf-8')
                except json.JSONDecodeError:
                    # If not valid JSON, send as plain text
                    data = message.encode('utf-8')
                
                sock.sendto(data, target)
                print(f"[Sent] {len(data)} bytes")
                
    except KeyboardInterrupt:
        print("\nStopping UDP sender")
    finally:
        sock.close()

def send_test_commands(port=8888):
    """Send a series of test commands"""
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
    
    print("Sending test command sequence...")
    
    for i, cmd in enumerate(test_commands):
        data = json.dumps(cmd).encode('utf-8')
        sock.sendto(data, target)
        print(f"[{i+1}/{len(test_commands)}] Sent: {cmd}")
        time.sleep(1)
    
    print("Test command sending completed")
    sock.close()

def main():
    if len(sys.argv) < 2:
        print("Usage:")
        print("  python test_liveion_udp.py listen    # Start UDP listener")
        print("  python test_liveion_udp.py send      # Start UDP sender")
        print("  python test_liveion_udp.py test      # Send test commands")
        print("  python test_liveion_udp.py both      # Start both listener and sender")
        return
    
    mode = sys.argv[1].lower()
    
    if mode == 'listen':
        udp_listener()
    elif mode == 'send':
        udp_sender()
    elif mode == 'test':
        send_test_commands()
    elif mode == 'both':
        # Start listener in background thread
        listener_thread = threading.Thread(target=udp_listener, daemon=True)
        listener_thread.start()
        time.sleep(1)  # Wait for listener to start
        
        # Run sender in main thread
        udp_sender()
    else:
        print(f"Unknown mode: {mode}")

if __name__ == "__main__":
    main()