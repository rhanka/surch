import json
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "surch_build_fixture_docs.py"
BASELINE = ROOT / "tests" / "matchid_parity" / "dev_deces_positive_capture_normalized.jsonl"


class SurchBuildFixtureDocsTests(unittest.TestCase):
    def test_builds_surch_docs_from_normalized_baseline(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            out_path = Path(temp_dir) / "docs.jsonl"
            result = subprocess.run(
                [
                    "python3",
                    str(SCRIPT),
                    "--baseline",
                    str(BASELINE),
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
            self.assertIn("DATE_NAISSANCE.raw", rows[0]["document"])
            self.assertIn("COMMUNE_NAISSANCE.raw", rows[0]["document"])
            self.assertIn("PAYS_NAISSANCE.raw", rows[0]["document"])


if __name__ == "__main__":
    unittest.main()
