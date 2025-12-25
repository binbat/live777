#!/usr/bin/env python3
"""
UDP Control Test Tool for live777 DataChannel Bridge

Usage:
    python tests/udp_control_test.py --host 127.0.0.1 --port 5005 --message "test"
"""

import socket
import argparse
import json
import time
import sys


def send_text_message(host: str, port: int, message: str):
    """Send a text message via UDP"""
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.sendto(message.encode('utf-8'), (host, port))
        print(f"✓ Sent text message: {message}")
    except Exception as e:
        print(f"✗ Error sending message: {e}", file=sys.stderr)
    finally:
        sock.close()


def send_json_message(host: str, port: int, data: dict):
    """Send a JSON message via UDP"""
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        message = json.dumps(data).encode('utf-8')
        sock.sendto(message, (host, port))
        print(f"✓ Sent JSON message: {json.dumps(data, indent=2)}")
    except Exception as e:
        print(f"✗ Error sending JSON: {e}", file=sys.stderr)
    finally:
        sock.close()


def send_binary_message(host: str, port: int, data: bytes):
    """Send binary data via UDP"""
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        sock.sendto(data, (host, port))
        print(f"✓ Sent binary message: {data.hex()}")
    except Exception as e:
        print(f"✗ Error sending binary: {e}", file=sys.stderr)
    finally:
        sock.close()


def send_ptz_command(host: str, port: int, action: str, **kwargs):
    """Send PTZ control command"""
    command = {"action": action, **kwargs}
    send_json_message(host, port, command)


def interactive_mode(host: str, port: int):
    """Interactive mode for sending commands"""
    print(f"\n=== UDP Control Interactive Mode ===")
    print(f"Target: {host}:{port}")
    print(f"Commands:")
    print(f"  text <message>     - Send text message")
    print(f"  json <json_string> - Send JSON message")
    print(f"  pan <left|right>   - Pan camera")
    print(f"  tilt <up|down>     - Tilt camera")
    print(f"  zoom <in|out>      - Zoom camera")
    print(f"  quit               - Exit")
    print()

    while True:
        try:
            cmd = input("udp> ").strip()
            if not cmd:
                continue

            parts = cmd.split(maxsplit=1)
            command = parts[0].lower()

            if command == "quit":
                break
            elif command == "text" and len(parts) > 1:
                send_text_message(host, port, parts[1])
            elif command == "json" and len(parts) > 1:
                try:
                    data = json.loads(parts[1])
                    send_json_message(host, port, data)
                except json.JSONDecodeError as e:
                    print(f"✗ Invalid JSON: {e}")
            elif command == "pan" and len(parts) > 1:
                direction = parts[1].lower()
                if direction in ["left", "right"]:
                    send_ptz_command(host, port, "pan", direction=direction, speed=50)
                else:
                    print("✗ Invalid direction. Use: left or right")
            elif command == "tilt" and len(parts) > 1:
                direction = parts[1].lower()
                if direction in ["up", "down"]:
                    send_ptz_command(host, port, "tilt", direction=direction, speed=50)
                else:
                    print("✗ Invalid direction. Use: up or down")
            elif command == "zoom" and len(parts) > 1:
                direction = parts[1].lower()
                if direction in ["in", "out"]:
                    send_ptz_command(host, port, "zoom", direction=direction, value=1)
                else:
                    print("✗ Invalid direction. Use: in or out")
            else:
                print(f"✗ Unknown command: {command}")

        except KeyboardInterrupt:
            print("\n\nExiting...")
            break
        except Exception as e:
            print(f"✗ Error: {e}")


def stress_test(host: str, port: int, count: int, interval: float):
    """Send multiple messages for stress testing"""
    print(f"\n=== Stress Test ===")
    print(f"Sending {count} messages to {host}:{port}")
    print(f"Interval: {interval}s\n")

    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    success = 0
    failed = 0

    try:
        for i in range(count):
            message = json.dumps({"seq": i, "timestamp": time.time()})
            try:
                sock.sendto(message.encode('utf-8'), (host, port))
                success += 1
                if (i + 1) % 100 == 0:
                    print(f"Sent {i + 1}/{count} messages...")
            except Exception as e:
                failed += 1
                print(f"✗ Failed to send message {i}: {e}")

            if interval > 0:
                time.sleep(interval)

    finally:
        sock.close()

    print(f"\n=== Results ===")
    print(f"Success: {success}")
    print(f"Failed: {failed}")
    print(f"Success rate: {success / count * 100:.2f}%")


def main():
    parser = argparse.ArgumentParser(
        description="UDP Control Test Tool for live777 DataChannel Bridge"
    )
    parser.add_argument(
        "--host",
        default="127.0.0.1",
        help="Target host (default: 127.0.0.1)"
    )
    parser.add_argument(
        "--port",
        type=int,
        default=5005,
        help="Target port (default: 5005)"
    )
    parser.add_argument(
        "--message",
        help="Send a single text message and exit"
    )
    parser.add_argument(
        "--json",
        help="Send a single JSON message and exit"
    )
    parser.add_argument(
        "--binary",
        help="Send binary data (hex string) and exit"
    )
    parser.add_argument(
        "--interactive",
        action="store_true",
        help="Enter interactive mode"
    )
    parser.add_argument(
        "--stress",
        type=int,
        metavar="COUNT",
        help="Stress test: send COUNT messages"
    )
    parser.add_argument(
        "--interval",
        type=float,
        default=0.01,
        help="Interval between messages in stress test (default: 0.01s)"
    )

    args = parser.parse_args()

    # Single message modes
    if args.message:
        send_text_message(args.host, args.port, args.message)
    elif args.json:
        try:
            data = json.loads(args.json)
            send_json_message(args.host, args.port, data)
        except json.JSONDecodeError as e:
            print(f"✗ Invalid JSON: {e}", file=sys.stderr)
            sys.exit(1)
    elif args.binary:
        try:
            data = bytes.fromhex(args.binary)
            send_binary_message(args.host, args.port, data)
        except ValueError as e:
            print(f"✗ Invalid hex string: {e}", file=sys.stderr)
            sys.exit(1)
    elif args.stress:
        stress_test(args.host, args.port, args.stress, args.interval)
    elif args.interactive:
        interactive_mode(args.host, args.port)
    else:
        # Default: show examples
        print("UDP Control Test Tool")
        print("\nExamples:")
        print(f"  {sys.argv[0]} --message 'Hello'")
        print(f"  {sys.argv[0]} --json '{{\"action\":\"pan\",\"direction\":\"left\"}}'")
        print(f"  {sys.argv[0]} --binary '010032'")
        print(f"  {sys.argv[0]} --interactive")
        print(f"  {sys.argv[0]} --stress 1000 --interval 0.01")
        print("\nUse --help for more options")


if __name__ == "__main__":
    main()
