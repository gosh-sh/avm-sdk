#!/usr/bin/env python3
"""Check tracked release text and messages in an explicit commit range."""

import argparse
from pathlib import Path
import re
import subprocess
import sys


RESTRICTED = re.compile(
    r"co[\x2d]authored[\x2d]by\b|generat\u0065d[ \t]+with\b|"
    r"(?:cl\u0061ude|anthr\u006fpic|cod\u0065x|open\u0061i|astr\u0061|"
    r"cop\u0069lot|chatg\u0070t|gem\u0069ni|deeps\u0065ek)|"
    r"(?<![a-z0-9])(?:l\u006cm|a\u0069)(?![a-z0-9])|"
    r"(?<![a-z0-9])gp\u0074(?:[-_.]?\d[\w.-]*)?(?![a-z0-9])|"
    r"\bartificial[ -]+intellig\u0065nce\b|\blarge[ -]+language[ -]+mod\u0065ls?\b",
    re.IGNORECASE,
)
CYRILLIC = re.compile(
    r"[\u0400-\u052f\u1c80-\u1c8f\u2de0-\u2dff\ua640-\ua69f\U0001e030-\U0001e08f]")
# Exact lines of intentional Unicode regression data, keyed by tracked path.
UNICODE_TEST_LINES = {}


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args], stderr=subprocess.PIPE)


def text(data):
    if data.startswith((b"\xff\xfe", b"\xfe\xff")):
        return data.decode("utf-16")
    if b"\0" in data:
        return None
    return data.decode("utf-8-sig")


def violations(value, label, allowed=()):
    result = []
    for number, line in enumerate(value.splitlines(), 1):
        if RESTRICTED.search(line):
            result.append(f"{label}:{number}: restricted wording")
        if CYRILLIC.search(line) and line not in allowed:
            result.append(f"{label}:{number}: unsupported script")
    return result


def check(root, base):
    base_sha = git(root, "rev-parse", "--verify", base + "^{commit}").decode().strip()
    head = git(root, "rev-parse", "--verify", "HEAD^{commit}").decode().strip()
    errors = []
    files = git(root, "ls-files", "-z").decode().split("\0")
    scanned = 0
    for name in filter(None, files):
        path = root / name
        value = path.readlink().as_posix() if path.is_symlink() else text(path.read_bytes())
        if value is not None:
            scanned += 1
            errors.extend(violations(name, "path " + name))
            errors.extend(violations(value, name, UNICODE_TEST_LINES.get(name, ())))
    commits = git(root, "rev-list", base_sha + ".." + head).decode().splitlines()
    for commit in commits:
        message = git(root, "show", "-s", "--format=%B", commit).decode("utf-8")
        errors.extend(violations(message, "commit " + commit[:12]))
    return errors, scanned, len(commits)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--base", default="HEAD^", help="exclusive base revision of the reviewed range")
    args = parser.parse_args()
    try:
        errors, files, commits = check(args.repo.resolve(), args.base)
        if errors:
            print("release text: FAIL", file=sys.stderr)
            for error in errors:
                print(error, file=sys.stderr)
            return 1
        print(f"release text: PASS ({files} text files, {commits} commits)")
        return 0
    except (OSError, UnicodeError, subprocess.CalledProcessError):
        print("release text: cannot inspect the complete tree and commit range", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
