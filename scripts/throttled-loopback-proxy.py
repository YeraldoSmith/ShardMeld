#!/usr/bin/env python3
"""One-shot loopback TCP proxy for deterministic interoperability tests."""

from __future__ import annotations

import argparse
import socket
import threading
import time


def parse_address(value: str) -> tuple[str, int]:
    host, separator, port = value.rpartition(":")
    if not separator or host not in {"127.0.0.1", "localhost"}:
        raise argparse.ArgumentTypeError("address must be loopback host:port")
    try:
        parsed_port = int(port)
    except ValueError as error:
        raise argparse.ArgumentTypeError("port must be an integer") from error
    if not 1 <= parsed_port <= 65535:
        raise argparse.ArgumentTypeError("port must be between 1 and 65535")
    return "127.0.0.1", parsed_port


def relay(source: socket.socket, destination: socket.socket) -> None:
    try:
        while payload := source.recv(64 * 1024):
            destination.sendall(payload)
    except OSError:
        pass
    finally:
        try:
            destination.shutdown(socket.SHUT_WR)
        except OSError:
            pass


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--listen", required=True, type=parse_address)
    parser.add_argument("--upstream", required=True, type=parse_address)
    parser.add_argument("--down-bytes-per-second", required=True, type=int)
    arguments = parser.parse_args()
    if arguments.down_bytes_per_second <= 0:
        parser.error("--down-bytes-per-second must be positive")

    with socket.create_server(arguments.listen, family=socket.AF_INET) as listener:
        client, _ = listener.accept()
        with client, socket.create_connection(arguments.upstream, timeout=10) as upstream:
            upstream.settimeout(None)
            sender = threading.Thread(target=relay, args=(client, upstream), daemon=True)
            sender.start()
            try:
                while payload := upstream.recv(4096):
                    client.sendall(payload)
                    time.sleep(len(payload) / arguments.down_bytes_per_second)
            except OSError:
                pass


if __name__ == "__main__":
    main()
