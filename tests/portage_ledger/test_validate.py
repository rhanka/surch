import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
VALIDATOR = ROOT / "tools" / "portage-ledger" / "validate.py"


def write_ticket(path: Path, **overrides: object) -> None:
    ticket = {
        "id": "LUCENE-store-DataInput-001",
        "title": "Port DataInput variable-length integer decoding",
        "owner": "StorageEngine",
        "priority": "Critical",
        "upstream_ref": {
            "repo": "lucene",
            "commit": "7691b7ef9cfe3b87178646f4f32b3854afa0a567",
            "files": ["lucene/core/src/java/org/apache/lucene/store/DataInput.java"],
            "symbols": ["readVInt", "readVLong", "readZLong"],
        },
        "parity_level": "P1 behavior",
        "dependencies": [],
        "allowed_paths": ["crates/surch-store/**", "tests/lucene_parity/**"],
        "forbidden_paths": ["crates/surch-api/**"],
        "golden_tests_required": [
            "Java fixture emits encoded bytes and expected decoded values",
            "Rust test consumes fixture and matches Java behavior",
        ],
        "gates": ["cargo test -p surch-store data_input"],
        "status": "discovered",
    }
    ticket.update(overrides)
    path.write_text(json.dumps(ticket, indent=2), encoding="utf-8")


class PortageLedgerValidatorTests(unittest.TestCase):
    def test_accepts_valid_ticket_directory(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tickets = Path(tmp)
            write_ticket(tickets / "valid.json")

            result = subprocess.run(
                [sys.executable, str(VALIDATOR), str(tickets)],
                cwd=ROOT,
                check=False,
                capture_output=True,
                text=True,
            )

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("validated 1 ticket", result.stdout)

    def test_rejects_ticket_without_golden_tests(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            tickets = Path(tmp)
            write_ticket(tickets / "missing-golden.json", golden_tests_required=[])

            result = subprocess.run(
                [sys.executable, str(VALIDATOR), str(tickets)],
                cwd=ROOT,
                check=False,
                capture_output=True,
                text=True,
            )

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("golden_tests_required", result.stderr)


if __name__ == "__main__":
    unittest.main()
