import json
import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "matchid_compile_deces_corpus.py"
SEED = ROOT / "tests" / "matchid_parity" / "matchid_request_seed.jsonl"


class MatchIdCompileDecesCorpusTests(unittest.TestCase):
    def test_compiles_seed_into_search_corpus(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            out_path = Path(temp_dir) / "compiled.jsonl"
            result = subprocess.run(
                [
                    "python3",
                    str(SCRIPT),
                    "--seed",
                    str(SEED),
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
            self.assertEqual(rows[0]["request"]["path"], "/deces/_search")
            must_clauses = rows[0]["request"]["json"]["query"]["bool"]["must"]
            self.assertTrue(any("DATE_NAISSANCE.raw" in json.dumps(clause) for clause in must_clauses))


if __name__ == "__main__":
    unittest.main()
