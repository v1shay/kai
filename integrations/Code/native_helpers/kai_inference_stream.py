"""Bounded inference reader with heartbeats during model load and prefill.
One reader exists only for an active inference; cancellation closes its socket.
"""
import contextlib
import queue
import socket
import threading
import time


def lines(opener, request, timeout=900, deadline=3600, heartbeat=10):
    items = queue.Queue(4)
    stopped = threading.Event()
    active = []

    def put(value):
        while not stopped.is_set():
            try:
                items.put(value, timeout=.2)
                return
            except queue.Full:
                pass

    def read():
        try:
            with opener.open(request, timeout=timeout) as response:
                active.append(response)
                while not stopped.is_set():
                    line = response.readline(8 * 1024 * 1024 + 1)
                    if not line:
                        break
                    if len(line) > 8 * 1024 * 1024:
                        raise ValueError('Inference event exceeds 8 MiB')
                    put(line)
            put(None)
        except Exception as error:
            put(error)

    threading.Thread(target=read, daemon=True).start()
    until = time.monotonic() + deadline
    try:
        while time.monotonic() < until:
            try:
                value = items.get(timeout=min(heartbeat, max(.01, until-time.monotonic())))
            except queue.Empty:
                yield b'\n'
                continue
            if value is None:
                return
            if isinstance(value, Exception):
                raise value
            yield value
        raise TimeoutError('Inference exceeded its one-hour execution deadline')
    finally:
        stopped.set()
        for response in active:
            with contextlib.suppress(Exception):
                response.fp.raw._sock.shutdown(socket.SHUT_RDWR)
            with contextlib.suppress(Exception):
                response.close()
