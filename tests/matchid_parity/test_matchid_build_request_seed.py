import json
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "matchid_build_request_seed.py"
CSV = ROOT.parent / "matchID" / "matchID" / "packages" / "deces-backend" / "tests" / "clients_test.csv"


class MatchIdBuildRequestSeedTests(unittest.TestCase):
    def test_builds_seed_file_from_matchid_csv(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            out_path = Path(temp_dir) / "seed.jsonl"
            result = subprocess.run(
                [
                    "python3",
                    str(SCRIPT),
                    "--csv",
                    str(CSV),
                    "--out",
                    str(out_path),
                    "--limit",
                    "2",
                ],
                capture_output=True,
                text=True,
                check=False,
            )

            self.assertEqual(result.returncode, 0)
            lines = [json.loads(line) for line in out_path.read_text(encoding="utf-8").splitlines()]
            self.assertEqual(len(lines), 2)
            self.assertEqual(lines[0]["case_id"], "clients-test-0001")
            self.assertEqual(lines[0]["matchid_request"]["lastName"], "CLIQUE")


if __name__ == "__main__":
    unittest.main()
