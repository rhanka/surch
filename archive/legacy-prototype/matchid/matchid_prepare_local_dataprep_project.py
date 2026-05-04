#!/usr/bin/env python3

import argparse
import shutil
from pathlib import Path


def replace_in_file(path: Path, old: str, new: str):
    content = path.read_text(encoding="utf-8")
    path.write_text(content.replace(old, new), encoding="utf-8")


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-project", required=True)
    parser.add_argument("--dataset-directory", required=True)
    parser.add_argument("--output-directory", required=True)
    parser.add_argument("--out", required=True)
    args = parser.parse_args(argv)

    source_project = Path(args.source_project)
    out_dir = Path(args.out)
    dataset_directory = Path(args.dataset_directory)
    output_directory = Path(args.output_directory)

    if out_dir.exists():
        shutil.rmtree(out_dir)
    shutil.copytree(source_project, out_dir)
    output_directory.mkdir(parents=True, exist_ok=True)

    replace_in_file(
        out_dir / "datasets" / "deces_src.yml",
        "connector: !ENV ${DATAGOUV_CONNECTOR}",
        "connector: localfiles",
    )

    replace_in_file(
        out_dir / "datasets" / "deces_csv.yml",
        "connector: s3",
        "connector: localout",
    )

    replace_in_file(
        out_dir / "datasets" / "deces_csv.yml",
        "table: deces.csv.gz",
        "table: deces.csv",
    )

    replace_in_file(
        out_dir / "recipes" / "deces_dataprep.yml",
        "dataset: deces_index",
        "dataset: deces_csv",
    )

    connectors_yaml = f"""connectors:
  localfiles:
    type: filesystem
    directory: {dataset_directory}
    chunk: 10000
  localout:
    type: filesystem
    directory: {output_directory}
    chunk: 10000
"""

    (out_dir / "localfiles.yml").write_text(connectors_yaml, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
