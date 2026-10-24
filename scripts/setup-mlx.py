#!/usr/bin/env python3
"""Explicit developer setup for the experimental MLX runtime. This step downloads packages."""
import argparse
import json
from pathlib import Path
import platform
import shutil
import subprocess
import sys

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--runtime-dir", type=Path, required=True)
args = parser.parse_args()
if platform.system() != "Darwin" or platform.machine() != "arm64":
    raise SystemExit("Experimental MLX runtime supports Apple Silicon only")
target = args.runtime_dir.resolve() / "mlx"
target.mkdir(parents=True, exist_ok=True)
python = target / "venv/bin/python3"
if not python.exists():
    subprocess.run([sys.executable, "-m", "venv", str(target / "venv")], check=True)
subprocess.run([str(python), "-m", "pip", "--isolated", "install", "--index-url", "https://pypi.org/simple", "mlx==0.31.2", "mlx-lm==0.31.3", "transformers==5.7.0", "huggingface-hub==1.13.0"], check=True)
shutil.copy2(Path(__file__).with_name("mlx-runner.py"), target / "runner.py")
(target / "build-info.json").write_text(json.dumps({"experimental": True, "mlx": "0.31.2", "mlx_lm": "0.31.3", "packaged_in_installer": False}) + "\n")
(target / "installed-packages.txt").write_bytes(subprocess.check_output([str(python), "-m", "pip", "freeze"]))
print("Experimental runtime ready. Download a curated MLX model separately in OmaScribe settings.")
