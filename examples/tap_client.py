#!/usr/bin/env python3
"""
Example Text Tap client for Termania.

Connects to the Termania text tap Unix socket and prints
terminal content as it updates. This lets external processes
monitor what's happening in any terminal pane.

Usage:
    python3 tap_client.py [socket_path]

The socket path defaults to /tmp/termania.sock
"""

import json
import socket
import sys


def main():
    sock_path = sys.argv[1] if len(sys.argv) > 1 else "/tmp/termania.sock"

    print(f"Connecting to Termania text tap at {sock_path}...")
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    sock.connect(sock_path)
    print("Connected! Listening for terminal output...\n")

    # Subscribe to all panes
    sock.sendall(b'{"subscribe": "all"}\n')

    buf = b""
    try:
        while True:
            data = sock.recv(65536)
            if not data:
                print("Connection closed")
                break

            buf += data
            while b"\n" in buf:
                line, buf = buf.split(b"\n", 1)
                try:
                    msg = json.loads(line)
                    pane = msg.get("pane", "?")
                    content = msg.get("content", "")
                    print(f"--- Pane {pane} ---")
                    print(content)
                except json.JSONDecodeError:
                    pass
    except KeyboardInterrupt:
        print("\nDisconnected.")
    finally:
        sock.close()


if __name__ == "__main__":
    main()
