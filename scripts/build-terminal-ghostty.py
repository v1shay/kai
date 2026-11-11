#!/usr/bin/env python3
"""Build Kai's lazily loaded macOS terminal adapter from a pinned upstream release."""
import hashlib
import os
from pathlib import Path
import platform
import subprocess
import sys
import tempfile
from zipfile import ZipFile

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / 'integrations/Code/native_helpers/TerminalGhostty.c'
VERSION = '1.0.0'
ARCHIVE = f'ghostty-vt-{VERSION}.xcframework.zip'
SHA256 = '2c32b7409b2b6c2ef6cfd606526de357762ad458bdc27134ad074e86506a8bc2'
MAC_SLICE = 'ghostty-vt.xcframework/macos-arm64_x86_64'
output = Path(sys.argv[1]).resolve()
cache = output.parent / 'ghostty-vt-cache'
cache.mkdir(parents=True, exist_ok=True)

architecture = platform.machine()
flags = [
    '-dynamiclib', '-O3', '-fvisibility=hidden', '-mmacosx-version-min=13.0',
    '-arch', architecture, '-install_name', '@rpath/libKaiTerminalGhostty.dylib',
    '-Wl,-dead_strip', '-Wl,-exported_symbol,_kai_terminal_ghostty_v1',
]
stamp = output.with_suffix('.sha256')
key = hashlib.sha256(SOURCE.read_bytes() + repr((flags, SHA256)).encode()).hexdigest()
if output.is_file() and stamp.is_file() and stamp.read_text() == key:
    sys.exit(0)

archive = cache / ARCHIVE
with tempfile.TemporaryDirectory(dir=cache) as scratch:
    scratch = Path(scratch)
    if not archive.exists():
        download = scratch / ARCHIVE
        url = f'https://release.files.ghostty.org/libghostty-vt/{VERSION}/{ARCHIVE}'
        subprocess.run(['curl', '-fsSL', '--retry', '2', '--max-time', '90', '-o', download, url], check=True)
        if hashlib.sha256(download.read_bytes()).hexdigest() != SHA256:
            raise SystemExit('Ghostty VT release checksum mismatch')
        os.replace(download, archive)
    if hashlib.sha256(archive.read_bytes()).hexdigest() != SHA256:
        raise SystemExit(f'Ghostty VT release checksum mismatch in {archive}')
    with ZipFile(archive) as package:
        for name in package.namelist():
            if name.startswith(MAC_SLICE + '/'):
                package.extract(name, scratch)
    library = scratch / MAC_SLICE
    staged = scratch / output.name
    subprocess.run(['xcrun', 'clang', *flags, '-I', library / 'Headers', SOURCE,
                    library / 'libghostty-vt.a', '-o', staged], check=True)
    os.replace(staged, output)
    stamp.write_text(key)
