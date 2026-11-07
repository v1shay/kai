#!/usr/bin/env python3
"""Sign local Kai builds with a persistent, private development identity.

The dedicated keychain is not added to the user's search list. Its certificate
is trusted only for code signing in the current user account; no system-wide
trust settings, browser/TLS trust, or privacy grants are changed. Distribution builds can set
KAI_SIGNING_IDENTITY and optionally KAI_SIGNING_KEYCHAIN instead.
"""
import argparse
import os
from pathlib import Path
import plistlib
import re
import secrets
import shlex
import subprocess
import tempfile

NAME = 'Kai Local Development'


def run(args, sensitive=()):
    result = subprocess.run([str(a) for a in args], capture_output=True, text=True)
    if result.returncode:
        message = result.stderr.strip() or result.stdout.strip()
        for secret in sensitive:
            message = message.replace(secret, '[redacted]')
        raise RuntimeError(f'{Path(args[0]).name}: {message}')
    return result.stdout.strip()


def local_identity():
    directory = Path.home() / 'Library/Application Support/Kai Build'
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    directory.chmod(0o700)
    keychain = directory / 'local-signing.keychain-db'
    password_file = directory / 'local-signing.password'
    if not keychain.exists():
        if not password_file.exists():
            fd = os.open(password_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            with os.fdopen(fd, 'w') as handle:
                handle.write(secrets.token_hex(32))
        password = password_file.read_text().strip()
        # security create-keychain can add the keychain to the user's search
        # list. Restore that list even when certificate creation/import fails.
        search = shlex.split(run(['/usr/bin/security', 'list-keychains', '-d', 'user']))
        try:
            run(['/usr/bin/security', 'create-keychain', '-p', password, keychain], [password])
        finally:
            run(['/usr/bin/security', 'list-keychains', '-d', 'user', '-s', *search])
    if not password_file.exists():
        raise RuntimeError('The local signing keychain password file is missing; refusing to replace the identity.')
    password = password_file.read_text().strip()
    run(['/usr/bin/security', 'unlock-keychain', '-p', password, keychain], [password])
    run(['/usr/bin/security', 'set-keychain-settings', '-lut', '21600', keychain])
    identities = run(['/usr/bin/security', 'find-identity', '-p', 'codesigning', keychain])
    matches = list(dict.fromkeys(re.findall(r'([0-9A-F]{40}) "' + re.escape(NAME) + r'"', identities)))
    if not matches:
        with tempfile.TemporaryDirectory(prefix='.certificate-', dir=directory) as temporary:
            work = Path(temporary)
            config = work / 'certificate.cnf'
            config.write_text('[req]\nprompt=no\ndistinguished_name=subject\nx509_extensions=extensions\n'
                              '[subject]\nCN=Kai Local Development\nO=Kai local builds\n'
                              '[extensions]\nbasicConstraints=critical,CA:TRUE\n'
                              'keyUsage=critical,digitalSignature,keyCertSign\n'
                              'extendedKeyUsage=critical,codeSigning\n'
                              'subjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid:always\n')
            run(['/usr/bin/openssl', 'req', '-new', '-x509', '-sha256', '-days', '3650', '-newkey', 'rsa:2048', '-nodes', '-config', config, '-keyout', work/'key.pem', '-out', work/'certificate.pem'])
            run(['/usr/bin/openssl', 'pkcs12', '-export', '-inkey', work/'key.pem', '-in', work/'certificate.pem', '-name', NAME, '-out', work/'identity.p12', '-passout', f'file:{password_file}'])
            run(['/usr/bin/security', 'import', work/'identity.p12', '-k', keychain, '-P', password, '-T', '/usr/bin/codesign'], [password])
            run(['/usr/bin/security', 'set-key-partition-list', '-S', 'apple-tool:,apple:', '-s', '-k', password, keychain], [password])
        identities = run(['/usr/bin/security', 'find-identity', '-p', 'codesigning', keychain])
        matches = list(dict.fromkeys(re.findall(r'([0-9A-F]{40}) "' + re.escape(NAME) + r'"', identities)))
    if len(matches) != 1:
        raise RuntimeError('Could not resolve a unique local Kai signing identity.')
    valid = run(['/usr/bin/security', 'find-identity', '-v', '-p', 'codesigning', keychain])
    if matches[0] not in valid:
        # Self-signed local identities need user-domain code-signing trust.
        # This does not install a system root or grant TLS/browser trust.
        certificate = directory / 'local-signing.certificate.pem'
        certificate.write_text(run(['/usr/bin/security', 'find-certificate', '-c', NAME, '-p', keychain]) + '\n')
        run(['/usr/bin/security', 'add-trusted-cert', '-r', 'trustRoot', '-p', 'codeSign', '-k', keychain, certificate])
    return matches[0], keychain


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('app', type=Path)
    args = parser.parse_args()
    app = args.app.resolve(strict=True)
    info = plistlib.loads((app/'Contents/Info.plist').read_bytes())
    identity = os.environ.get('KAI_SIGNING_IDENTITY')
    keychain = os.environ.get('KAI_SIGNING_KEYCHAIN')
    if not identity:
        identity, keychain = local_identity()
    signing = ['/usr/bin/codesign', '--force', '--sign', identity, '--timestamp=none']
    if keychain:
        signing += ['--keychain', str(keychain)]
    resources = app/'Contents/Resources'
    for library in sorted(resources.glob('*.dylib')):
        run([*signing, library])
    helper = resources/'kai-ninja-host'
    if helper.is_file():
        run([*signing, helper])
    run([*signing, app])
    run(['/usr/bin/codesign', '--verify', '--deep', '--strict', app])
    # codesign prints requirements to stdout on some SDKs and stderr on others.
    result = subprocess.run(['/usr/bin/codesign', '-d', '-r-', str(app)], capture_output=True, text=True)
    requirement = result.stdout + result.stderr
    if identity != '-' and ('designated =>' not in requirement or 'certificate' not in requirement and 'anchor' not in requirement):
        raise RuntimeError('The signed app has no certificate-backed designated requirement.')
    print('Verified certificate-backed Kai app identity' if identity != '-' else 'Verified explicitly requested ad-hoc app signature')
    print('Bundle:', info['CFBundleIdentifier'])


if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, OSError, ValueError) as error:
        raise SystemExit(str(error))
