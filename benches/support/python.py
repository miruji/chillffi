# Python side of benches/warm.rs and benches/cold.rs, loaded into the benchmark process through pyo3.
# Only the operations live here. Timing, iteration counts and the table belong to divan.
# Each operation means exactly what it means in the Rust rows: ctypes, and hand-made fork+socketpair isolation.
import _ctypes
import ctypes
import os
import socket
import struct

# set by support/mod.rs right after loading: libcPath
payloadSize = 1 << 16
payload = b"\xa5" * payloadSize


def loadToupper():
    libc = ctypes.CDLL(libcPath)
    libc.toupper.argtypes = [ctypes.c_int]
    libc.toupper.restype = ctypes.c_int
    return libc.toupper


# ------------------------------------------------------------------------------------ in-process

def ctypesOneShot():
    libc = ctypes.CDLL(libcPath)
    libc.toupper.argtypes = [ctypes.c_int]
    libc.toupper.restype = ctypes.c_int
    value = libc.toupper(97)
    _ctypes.dlclose(libc._handle)  # ctypes never closes on its own; libloading does on drop
    return value


def makeCtypesPayload():
    buffer = ctypes.create_string_buffer(payloadSize)

    def operation():
        ctypes.memmove(buffer, payload, payloadSize)
        return buffer.raw

    return operation


# ------------------------------------------------------------------------------------ isolated

def recvExact(sock, size):
    chunks = []
    remaining = size
    while remaining:
        chunk = sock.recv(remaining)
        if not chunk:
            return None
        chunks.append(chunk)
        remaining -= len(chunk)
    return chunks[0] if len(chunks) == 1 else b"".join(chunks)


def serveToupper(sock, size):
    toupper = loadToupper()
    while (data := recvExact(sock, size)) is not None:
        sock.sendall(struct.pack("i", toupper(struct.unpack("i", data)[0])))


def serveBuffer(sock, size):
    """Same shape as chillffi's memory API: opcode 1 = write (payload follows, 1-byte ack), opcode 2 = read."""
    buffer = ctypes.create_string_buffer(size)
    while (opcode := recvExact(sock, 1)) is not None:
        if opcode == b"\x01":
            data = recvExact(sock, size)
            if data is None:
                break
            ctypes.memmove(buffer, data, size)
            sock.sendall(b"\x01")
        else:
            sock.sendall(buffer.raw)


def spawnWorker(size, serve):
    parent, child = socket.socketpair()
    pid = os.fork()
    if pid == 0:
        parent.close()
        try:
            serve(child, size)
        finally:
            os._exit(0)
    child.close()
    return parent, pid


def finishWorker(sock, pid):
    sock.close()
    os.waitpid(pid, 0)


def makeForkCall():
    """Returns (operation, finish): one round trip to a persistent isolated worker, and its teardown."""
    sock, pid = spawnWorker(4, serveToupper)
    request = struct.pack("i", 97)
    sock.sendall(request)
    assert struct.unpack("i", recvExact(sock, 4))[0] == 65

    def operation():
        sock.sendall(request)
        recvExact(sock, 4)

    return operation, lambda: finishWorker(sock, pid)


def forkOneShot():
    parent, child = socket.socketpair()
    pid = os.fork()
    if pid == 0:
        parent.close()
        try:
            child.sendall(struct.pack("i", loadToupper()(97)))
        finally:
            os._exit(0)
    child.close()
    value = struct.unpack("i", recvExact(parent, 4))[0]
    finishWorker(parent, pid)
    assert value == 65
    return value


def makeForkPayload():
    sock, pid = spawnWorker(payloadSize, serveBuffer)
    request = b"\x01" + payload

    def operation():
        sock.sendall(request)
        recvExact(sock, 1)
        sock.sendall(b"\x02")
        recvExact(sock, payloadSize)

    return operation, lambda: finishWorker(sock, pid)
