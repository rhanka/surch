import json
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "matchid_backend_normalize.py"
SOURCE = ROOT / "tests" / "matchid_parity" / "dev_deces_positive_capture.jsonl"


class MatchIdBackendNormalizeTests(unittest.TestCase):
    def test_normalizes_backend_capture_into_comparator_shape(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            out_path = Path(temp_dir) / "normalized.jsonl"
            result = subprocess.run(
                [
                    "python3",
                    str(SCRIPT),
                    "--in",
                    str(SOURCE),
                    "--out",
                    str(out_path),
                ],
                capture_output=True,
                text=True,
                check=False,
            )

            self.assertEqual(result.returncode, 0)
            rows = [json.loads(line) for line in out_path.read_text(encoding="utf-8").splitlines()]
            self.assertGreaterEqual(len(rows), 1)
            self.assertIn("hits", rows[0]["response"])
            self.assertIn("total", rows[0]["response"]["hits"])
            source = rows[0]["response"]["hits"]["hits"][0]["_source"]
            self.assertIn("UID", source)
            self.assertIn("NOM", source)
            self.assertIn("PRENOM", source)
            self.assertIn("DATE_NAISSANCE", source)


if __name__ == "__main__":
    unittest.main()
