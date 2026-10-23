#!/usr/bin/env python3
"""Integration check using only Python's standard library. Requires a running server."""
import base64
import hashlib
import json
import os
import socket
import ssl
import struct
import sys
from urllib.parse import urlparse


class WS:
    def __init__(self, url):
        parsed = urlparse(url)
        if parsed.scheme not in ("ws", "wss"):
            raise ValueError("Use ws:// or wss://")
        host = parsed.hostname
        port = parsed.port or (443 if parsed.scheme == "wss" else 80)
        sock = socket.create_connection((host, port), timeout=10)
        self.sock = ssl.create_default_context().wrap_socket(sock, server_hostname=host) if parsed.scheme == "wss" else sock
        key = base64.b64encode(os.urandom(16)).decode()
        path = parsed.path or "/"
        if parsed.query:
            path += "?" + parsed.query
        request = (
            f"GET {path} HTTP/1.1\r\n"
            f"Host: {host}:{port}\r\n"
            "Upgrade: websocket\r\nConnection: Upgrade\r\n"
            f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n"
        )
        self.sock.sendall(request.encode())
        head = b""
        while b"\r\n\r\n" not in head:
            part = self.sock.recv(1)
            if not part:
                raise RuntimeError("Connection closed during handshake")
            head += part
            if len(head) > 8192:
                raise RuntimeError("Oversized HTTP response")
        status, *headers = head.decode().split("\r\n")
        if " 101 " not in status:
            raise RuntimeError(f"WebSocket upgrade failed: {status}")
        values = dict((k.strip().lower(), v.strip()) for k, v in (h.split(":", 1) for h in headers if ":" in h))
        accept = values.get("sec-websocket-accept", "")
        expected = base64.b64encode(hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest()).decode()
        if accept != expected:
            raise RuntimeError("Invalid handshake response")

    def send(self, message):
        data = json.dumps(message).encode()
        mask = os.urandom(4)
        hdr = b"\x81"
        if len(data) < 126:
            hdr += bytes([0x80 | len(data)])
        elif len(data) < 65536:
            hdr += b"\xfe" + struct.pack("!H", len(data))
        else:
            hdr += b"\xff" + struct.pack("!Q", len(data))
        self.sock.sendall(hdr + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))

    def read_n(self, n):
        data = b""
        while len(data) < n:
            chunk = self.sock.recv(n - len(data))
            if not chunk:
                raise RuntimeError("Socket closed")
            data += chunk
        return data

    def recv(self):
        while True:
            f, length = self.read_n(2)
            opcode = f & 15
            masked = length & 128
            n = length & 127
            if n == 126:
                n = struct.unpack("!H", self.read_n(2))[0]
            elif n == 127:
                n = struct.unpack("!Q", self.read_n(8))[0]
            mask = self.read_n(4) if masked else b""
            payload = self.read_n(n)
            if mask:
                payload = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
            if opcode == 8:
                raise RuntimeError("WebSocket closed by server")
            if opcode == 9:
                self._control_frame(10, payload)
                continue
            if opcode == 1:
                return json.loads(payload)

    def _control_frame(self, opcode, data):
        mask = os.urandom(4)
        self.sock.sendall(bytes([0x80 | opcode, 0x80 | len(data)]) + mask + bytes(b ^ mask[i % 4] for i, b in enumerate(data)))

    def close(self):
        self.sock.close()


def main():
    url = sys.argv[1] if len(sys.argv) > 1 else "ws://127.0.0.1:8080/ws"
    alice = WS(url)
    bob = WS(url)
    outsider = WS(url)
    try:
        alice.send({"type": "create_room", "name": "Alice"})
        created = alice.recv()
        assert created["type"] == "room_created", created
        room_id = created["room_id"]
        alice_id = created["peer_id"]

        bob.send({"type": "join_room", "room_id": room_id, "name": "Bob"})
        joined = bob.recv()
        assert joined["type"] == "room_joined", joined
        assert joined["peers"] == [{"id": alice_id, "name": "Alice"}]
        event = alice.recv()
        assert event["type"] == "peer_joined", event

        # A peer outside the room cannot send SDP/ICE to participants.
        outsider.send({"type": "signal", "to": alice_id, "payload": {"kind": "offer", "sdp": "fake"}})
        rejected = outsider.recv()
        assert rejected["type"] == "error" and rejected["code"] == "not_joined", rejected

        # Forwarding is restricted to members of the same room.
        bob.send({"type": "signal", "to": alice_id, "payload": {"kind": "offer", "sdp": "fake"}})
        forwarded = alice.recv()
        assert forwarded["type"] == "signal" and forwarded["payload"]["sdp"] == "fake", forwarded
        assert forwarded["from"] == joined["peer_id"]
        print("OK: create, join, peer_joined, unauthorized signal rejection and signal forwarding")
    finally:
        for client in (alice, bob, outsider):
            client.close()


if __name__ == "__main__":
    main()
