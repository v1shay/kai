"""Resolve the pinned Ghostty Metal archive for the native workspace."""
import hashlib
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from zipfile import ZipFile

checksum = "1f5cd5cebcca5181ea5aaea008252a2b622ba1e94471601cf896ee125e9a5902"
url = "https://github.com/Lakr233/libghostty-spm/releases/download/upstream.042b16b6146a/GhosttyKit.xcframework.zip"
root = Path(sys.argv[1]).resolve()
slice_name = "GhosttyKit.xcframework/macos-arm64_x86_64"
destination = root / slice_name
stamp = root / checksum
if not (stamp.is_file() and (destination / "libghostty.a").is_file()
        and (destination / "Headers/libghostty/module.modulemap").is_file()):
    root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=root) as scratch:
        archive = Path(scratch) / "GhosttyKit.zip"
        cached = root.parent / "ghostty-kitty.zip"
        if cached.is_file():
            shutil.copyfile(cached, archive)
        else:
            subprocess.run(["curl", "-fsSL", "--retry", "2", "-o", str(archive), url], check=True)
        with archive.open("rb") as stream:
            if hashlib.file_digest(stream, "sha256").hexdigest() != checksum:
                raise SystemExit("Ghostty Metal archive checksum mismatch")
        with ZipFile(archive) as package:
            for filename in ("libghostty.a", "Headers/libghostty/ghostty.h", "Headers/libghostty/module.modulemap"):
                package.extract(f"{slice_name}/{filename}", root)
        stamp.touch()
print(destination)
