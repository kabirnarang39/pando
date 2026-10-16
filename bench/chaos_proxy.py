#!/usr/bin/env python3
"""TCP proxy that loses replies: forwards LISTEN->TARGET pairs and, with a small probability
for each chunk the server sends back, cuts the connection. Requests that already reached
the server may have been applied, but their answers never arrive.

    python3 bench/chaos_proxy.py 7451:7441 7452:7442 7453:7443 [--drop 0.01]
"""
import argparse
import asyncio
import random


async def pipe(reader, writer, drop):
    try:
        while data := await reader.read(65536):
            if drop and random.random() < drop:
                break
            writer.write(data)
            await writer.drain()
    except (ConnectionError, OSError):
        pass
    finally:
        writer.close()


async def handle(client_r, client_w, port, drop):
    try:
        server_r, server_w = await asyncio.open_connection("127.0.0.1", port)
    except OSError:
        client_w.close()
        return
    await asyncio.gather(pipe(client_r, server_w, 0), pipe(server_r, client_w, drop))


async def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("pairs", nargs="+", help="LISTEN:TARGET")
    ap.add_argument("--drop", type=float, default=0.01)
    a = ap.parse_args()
    servers = []
    for pair in a.pairs:
        listen, target = (int(x) for x in pair.split(":"))
        servers.append(await asyncio.start_server(lambda r, w, t=target: handle(r, w, t, a.drop), "127.0.0.1", listen))
    await asyncio.gather(*(s.serve_forever() for s in servers))


asyncio.run(main())
