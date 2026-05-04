import json
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "matchid_performance_parity.py"
BASELINE = ROOT / "tests" / "matchid_parity" / "elastic_summary.json"
GOOD = ROOT / "tests" / "matchid_parity" / "surch_summary_good.json"
BAD = ROOT / "tests" / "matchid_parity" / "surch_summary_bad.json"


class MatchIdPerformanceParityTests(unittest.TestCase):
    def test_compare_accepts_summary_within_threshold(self):
        result = subprocess.run(
            [
                "python3",
                str(SCRIPT),
                "compare",
                "--baseline",
                str(BASELINE),
                "--candidate",
                str(GOOD),
                "--require-throughput-no-worse",
            ],
            capture_output=True,
            text=True,
            check=False,
        )

        self.assertEqual(result.returncode, 0)
        self.assertEqual(json.loads(result.stdout)["failures"], [])

    def test_compare_rejects_summary_outside_threshold(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            report_path = Path(temp_dir) / "perf-report.json"
            result = subprocess.run(
                [
                    "python3",
                    str(SCRIPT),
                    "compare",
                    "--baseline",
                    str(BASELINE),
                    "--candidate",
                    str(BAD),
                    "--require-throughput-no-worse",
                    "--report",
                    str(report_path),
                ],
                capture_output=True,
                text=True,
                check=False,
            )

            self.assertEqual(result.returncode, 1)
            report = json.loads(report_path.read_text(encoding="utf-8"))
            self.assertIn("error_rate", report["failures"])
            self.assertIn("p95", report["failures"])
            self.assertIn("p99", report["failures"])
            self.assertIn("throughput_rps", report["failures"])


if __name__ == "__main__":
    unittest.main()
