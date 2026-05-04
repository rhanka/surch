import subprocess
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "matchid_prepare_local_dataprep_project.py"
SOURCE_PROJECT = (
    ROOT.parent
    / "matchID"
    / "matchID"
    / "packages"
    / "deces-dataprep"
    / "projects"
    / "deces-dataprep"
)


class MatchIdPrepareLocalDataprepProjectTests(unittest.TestCase):
    def test_prepares_local_filesystem_dataprep_project(self):
        with tempfile.TemporaryDirectory() as temp_dir:
            temp_path = Path(temp_dir)
            out_dir = temp_path / "project"
            data_dir = temp_path / "data"
            output_dir = temp_path / "output"
            data_dir.mkdir()

            result = subprocess.run(
                [
                    "python3",
                    str(SCRIPT),
                    "--source-project",
                    str(SOURCE_PROJECT),
                    "--dataset-directory",
                    str(data_dir),
                    "--output-directory",
                    str(output_dir),
                    "--out",
                    str(out_dir),
                ],
                capture_output=True,
                text=True,
                check=False,
            )

            self.assertEqual(result.returncode, 0)
            self.assertTrue((out_dir / "localfiles.yml").exists())
            self.assertIn("connector: localfiles", (out_dir / "datasets" / "deces_src.yml").read_text())
            self.assertIn("dataset: deces_csv", (out_dir / "recipes" / "deces_dataprep.yml").read_text())


if __name__ == "__main__":
    unittest.main()
