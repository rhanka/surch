import json
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from scripts.matchid_elastic_parity import replay_case


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "matchid_elastic_parity.py"
CORPUS = ROOT / "tests" / "matchid_parity" / "sample_corpus.jsonl"
BASELINE = ROOT / "tests" / "matchid_parity" / "elastic_capture.jsonl"
IDENTICAL = ROOT / "tests" / "matchid_parity" / "surch_capture_identical.jsonl"
DIFF = ROOT / "tests" / "matchid_parity" / "surch_capture_diff.jsonl"


class MatchIdElasticParityTests(unittest.TestCase):
    def test_compare_returns_zero_for_identical_captures(self):
        result = subprocess.run(
            [
                "python3",
                str(SCRIPT),
                "compare",
                "--corpus",
                str(CORPUS),
                "--baseline",
                str(BASELINE),
                "--candidate",
                str(IDENTICAL),
            ],
            capture_output=True,
            text=True,
            check=False,
        )

        self.assertEqual(result.returncode, 0)
        self.assertEqual(json.loads(result.stdout), [])

    def test_compare_reports_diffs_and_nonzero_exit(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            report_path = Path(temp_dir) / "report.json"
            result = subprocess.run(
                [
                    "python3",
                    str(SCRIPT),
                    "compare",
                    "--corpus",
                    str(CORPUS),
                    "--baseline",
                    str(BASELINE),
                    "--candidate",
                    str(DIFF),
                    "--report",
                    str(report_path),
                ],
                capture_output=True,
                text=True,
                check=False,
            )

            self.assertEqual(result.returncode, 1)
            report = json.loads(report_path.read_text(encoding="utf-8"))
            self.assertGreaterEqual(len(report), 1)
            diff_categories = {diff for row in report for diff in row["diffs"]}
            self.assertIn("status_mismatch", diff_categories)
            self.assertIn("total_mismatch", diff_categories)

    def test_replay_case_via_docker_parses_response(self):
        case = {
            "case_id": "docker-case",
            "request": {
                "method": "POST",
                "path": "/deces/_search",
                "json": {"query": {"term": {"status": "published"}}},
            },
        }

        with mock.patch("scripts.matchid_elastic_parity.subprocess.run") as mocked_run:
            mocked_run.return_value = subprocess.CompletedProcess(
                args=["docker"],
                returncode=0,
                stdout='{"hits":{"total":{"value":1}}}\n200',
                stderr="",
            )

            capture = replay_case("http://unused", case, docker_container="deces-elasticsearch")

        self.assertEqual(capture["http_status"], 200)
        self.assertEqual(capture["response"]["hits"]["total"]["value"], 1)


if __name__ == "__main__":
    unittest.main()
