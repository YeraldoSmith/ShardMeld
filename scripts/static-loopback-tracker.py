#!/usr/bin/env python3
"""Minimal one-shot HTTP tracker returning explicit loopback peers."""

from __future__ import annotations

import argparse
import ipaddress
import socket
import struct
import urllib.parse


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


def bencode_response(peers: list[tuple[str, int]]) -> bytes:
    compact = compact_peers(peers)
    return b"d8:intervali60e5:peers" + str(len(compact)).encode() + b":" + compact + b"e"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--listen", required=True, type=parse_address)
    parser.add_argument("--peer", required=True, action="append", type=parse_address)
    parser.add_argument("--requests", type=int, default=2)
    arguments = parser.parse_args()
    if arguments.requests <= 0:
        parser.error("--requests must be positive")

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
                peers = [] if query.get("event") == ["stopped"] else arguments.peer
                body = bencode_response(peers)
                headers = (
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: "
                    + str(len(body)).encode()
                    + b"\r\nConnection: close\r\n\r\n"
                )
                connection.sendall(headers + body)


if __name__ == "__main__":
    main()
