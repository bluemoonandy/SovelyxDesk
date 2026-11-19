"""Windows-only P2 UI checks with a synthetic, loopback-only VNC server."""
import argparse
from pathlib import Path
import select
import socket
import struct
import subprocess
import threading
import time

FORMAT = struct.pack("!BBBBHHHBBB3x", 32, 24, 0, 1, 255, 255, 255, 16, 8, 0)


def read(connection, length):
    data = bytearray()
    while len(data) < length:
        block = connection.recv(length - len(data))
        if not block:
            raise EOFError("Viewer disconnected")
        data.extend(block)
    return bytes(data)


class SessionFixture:
    def __init__(self):
        self.listener = socket.socket()
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen(2)
        self.listener.settimeout(90)
        self.port = self.listener.getsockname()[1]
        self.stop = threading.Event()
        self.error = None

    def run(self):
        try:
            # First attempt deliberately fails without reading any credentials.
            connection, address = self.listener.accept()
            assert address[0] == "127.0.0.1"
            with connection:
                time.sleep(3)
            connection, address = self.listener.accept()
            assert address[0] == "127.0.0.1"
            with connection:
                connection.settimeout(15)
                connection.sendall(b"RFB 003.008\n")
                assert read(connection, 12) == b"RFB 003.008\n"
                connection.sendall(b"\x01\x02")
                assert read(connection, 1) == b"\x02"
                connection.sendall(bytes(range(16)))
                read(connection, 16)  # Synthetic challenge; no real credential is used.
                connection.sendall(bytes(4))
                read(connection, 1)
                connection.sendall(struct.pack("!HH", 1280, 720) + FORMAT + struct.pack("!I", 15) + b"P2 UI test Mac ")
                frame = ((b"\x70\x40\x20\0" * 640 + b"\xa0\x70\x30\0" * 640) * 360
                         + (b"\x80\x60\x40\0" * 640 + b"\xc0\xa0\x60\0" * 640) * 360)
                while not self.stop.is_set():
                    if not select.select([connection], [], [], 0.1)[0]:
                        continue
                    kind = read(connection, 1)[0]
                    if kind == 0:
                        read(connection, 19)
                    elif kind == 2:
                        count = struct.unpack("!H", read(connection, 3)[1:])[0]
                        read(connection, count * 4)
                    elif kind == 3:
                        read(connection, 9)
                        connection.sendall(bytes(3) + b"\x01" + struct.pack("!HHHHi", 0, 0, 1280, 720, 0) + frame)
                        time.sleep(0.1)
                    elif kind == 4:
                        read(connection, 7)
                    elif kind == 5:
                        read(connection, 5)
                    else:
                        raise AssertionError(f"Unexpected client message: {kind}")
        except (EOFError, ConnectionResetError, ConnectionAbortedError, BrokenPipeError):
            pass  # The UI closes its owned session during cleanup.
        except Exception as error:
            if not self.stop.is_set():
                self.error = error
        finally:
            self.listener.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--executable", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--language", default="en", choices=["en", "zh-CN"])
    args = parser.parse_args()
    fixture = SessionFixture()
    worker = threading.Thread(target=fixture.run, daemon=True)
    worker.start()
    try:
        subprocess.run([
            "pwsh", "-NoProfile", "-File", str(Path(__file__).with_name("Capture-Readme.ps1")),
            "-Language", args.language, "-Executable", args.executable, "-OutputDirectory", args.output,
            "-PreviewTheme", "light", "-DeviceCount", "6", "-SessionPort", str(fixture.port),
        ], check=True, timeout=180)
        if fixture.error:
            raise fixture.error
    finally:
        fixture.stop.set()
        worker.join(timeout=2)
