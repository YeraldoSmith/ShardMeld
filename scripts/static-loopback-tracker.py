#!/usr/bin/env python3
"""Minimal one-shot HTTP tracker returning explicit loopback peers."""

from __future__ import annotations

import argparse
import ipaddress
import json
import socket
import struct
import urllib.parse
from pathlib import Path


def parse_address(value: str) -> tuple[str, int]:
    host, separator, port = value.rpartition(":")
    if not separator:
        raise argparse.ArgumentTypeError("address must be host:port")
    try:
        address = ipaddress.ip_address(host)
        parsed_port = int(port)
    except ValueError as error:
        raise argparse.ArgumentTypeError("invalid IP address or port") from error
    if not address.is_loopback or address.version != 4:
        raise argparse.ArgumentTypeError("only IPv4 loopback addresses are allowed")
    if not 1 <= parsed_port <= 65535:
        raise argparse.ArgumentTypeError("port must be between 1 and 65535")
    return str(address), parsed_port


def compact_peers(peers: list[tuple[str, int]]) -> bytes:
    return b"".join(socket.inet_aton(host) + struct.pack("!H", port) for host, port in peers)


def bencode_response(peers: list[tuple[str, int]], interval: int) -> bytes:
    compact = compact_peers(peers)
    return (
        b"d8:intervali"
        + str(interval).encode()
        + b"e5:peers"
        + str(len(compact)).encode()
        + b":"
        + compact
        + b"e"
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--listen", required=True, type=parse_address)
    parser.add_argument("--peer", required=True, action="append", type=parse_address)
    parser.add_argument("--requests", type=int, default=2)
    parser.add_argument("--interval", type=int, default=60)
    parser.add_argument("--log-json", type=Path)
    arguments = parser.parse_args()
    if arguments.requests <= 0:
        parser.error("--requests must be positive")
    if arguments.interval <= 0:
        parser.error("--interval must be positive")

    observations = []
    with socket.create_server(arguments.listen, family=socket.AF_INET) as listener:
        for _ in range(arguments.requests):
            connection, _ = listener.accept()
            with connection:
                request = bytearray()
                while b"\r\n\r\n" not in request:
                    payload = connection.recv(4096)
                    if not payload:
                        break
                    request.extend(payload)
                first_line = bytes(request).split(b"\r\n", 1)[0]
                target = first_line.split(b" ", 2)[1].decode("ascii", "strict")
                query = urllib.parse.parse_qs(urllib.parse.urlsplit(target).query)
                observations.append(
                    {
                        "event": query.get("event", [None])[0],
                        "port": int(query["port"][0]),
                        "uploaded": int(query["uploaded"][0]),
                        "downloaded": int(query["downloaded"][0]),
                        "left": int(query["left"][0]),
                    }
                )
                peers = [] if query.get("event") == ["stopped"] else arguments.peer
                body = bencode_response(peers, arguments.interval)
                headers = (
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: "
                    + str(len(body)).encode()
                    + b"\r\nConnection: close\r\n\r\n"
                )
                connection.sendall(headers + body)
    if arguments.log_json:
        arguments.log_json.write_text(
            json.dumps(observations, indent=2) + "\n", encoding="utf-8"
        )


if __name__ == "__main__":
    main()
