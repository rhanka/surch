import json
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
import subprocess


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "matchid_backend_capture.py"


class _Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = json.dumps({"response": {"total": 1, "persons": []}}).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(body)

    def do_POST(self):
        _ = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        body = json.dumps({"response": {"total": 1, "persons": []}}).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args, **kwargs):
        return


class MatchIdBackendCaptureTests(unittest.TestCase):
    def test_captures_get_and_post_from_seed(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            seed_path = Path(temp_dir) / "seed.jsonl"
            out_path = Path(temp_dir) / "capture.jsonl"
            seed_path.write_text(
                json.dumps(
                    {
                        "case_id": "seed-1",
                        "matchid_request": {
                            "firstName": "MARCEL",
                            "lastName": "CLIQUE",
                            "birthDate": "01/09/1955",
                            "fuzzy": False,
                        },
                    }
                )
                + "\n",
                encoding="utf-8",
            )

            server = HTTPServer(("127.0.0.1", 0), _Handler)
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()

            try:
                base_url = f"http://127.0.0.1:{server.server_port}"
                result = subprocess.run(
                    [
                        "python3",
                        str(SCRIPT),
                        "--seed",
                        str(seed_path),
                        "--base-url",
                        base_url,
                        "--out",
                        str(out_path),
                    ],
                    capture_output=True,
                    text=True,
                    check=False,
                )
            finally:
                server.shutdown()
                thread.join(timeout=5)
                server.server_close()

            self.assertEqual(result.returncode, 0)
            rows = [json.loads(line) for line in out_path.read_text(encoding="utf-8").splitlines()]
            self.assertEqual(len(rows), 1)
            self.assertEqual(rows[0]["captures"]["get"]["http_status"], 200)
            self.assertEqual(rows[0]["captures"]["post"]["http_status"], 200)


if __name__ == "__main__":
    unittest.main()
