"""Black-box HTTP/1.1 regression check with a disposable controller and node."""
import base64
import hashlib
import http.client
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import unittest


@unittest.skipUnless(os.environ.get("YJ_TEST_CONTROLLER_BINARY"), "requires a built controller")
class AgentUpgrade(unittest.TestCase):
    def test_upgrade_and_regular_connection_headers(self):
        with tempfile.TemporaryDirectory() as directory:
            state = Path(directory)
            with socket.socket() as reservation:
                reservation.bind(("127.0.0.1", 0))
                port = reservation.getsockname()[1]
            address = f"127.0.0.1:{port}"
            command = [os.environ["YJ_TEST_CONTROLLER_BINARY"], "-web", "-listen", address,
                       "-origin", "https://" + address, "-state", directory]
            subprocess.run(command + ["-init"], check=True, stdout=subprocess.DEVNULL)
            token = os.urandom(32).hex()
            (state / "app.key").write_bytes(os.urandom(32))
            nodes = state / "nodes.json"
            data = json.loads(nodes.read_bytes())
            data["nodes"] = [{"visible": True, "public": {"id": "upgrade-test"}}]
            data["secrets"] = {"upgrade-test": {"token_hash": hashlib.sha256(token.encode()).hexdigest()}}
            nodes.write_text(json.dumps(data))
            proc = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            try:
                deadline = time.monotonic() + 10
                while True:
                    try:
                        with socket.create_connection(("127.0.0.1", port), timeout=1):
                            break
                    except OSError:
                        if proc.poll() is not None or time.monotonic() >= deadline:
                            self.fail("fixture controller failed to start")
                        time.sleep(0.05)
                connection = http.client.HTTPConnection("127.0.0.1", port, timeout=3)
                try:
                    connection.request("GET", "/api/public/nodes")
                    reply = connection.getresponse()
                    self.assertEqual(reply.status, 200)
                    self.assertEqual(reply.getheader("Connection").lower(), "close")
                    reply.read()
                    key = base64.b64encode(os.urandom(16)).decode()
                    connection.request("GET", "/api/agent", headers={
                        "Connection": "Upgrade", "Upgrade": "websocket", "Sec-WebSocket-Key": key,
                        "Sec-WebSocket-Version": "13", "Authorization": "Bearer " + token,
                        "X-Probe-Node": "upgrade-test",
                    })
                    reply = connection.getresponse()
                    self.assertEqual(reply.status, 101)
                    self.assertEqual(reply.getheader("Upgrade").lower(), "websocket")
                    self.assertEqual(reply.getheader("Connection").lower(), "upgrade")
                    reply.close()
                finally:
                    connection.close()
            finally:
                proc.terminate()
                proc.wait(timeout=10)
