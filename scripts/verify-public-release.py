#!/usr/bin/env python3
"""Verify GitHub release assets or prepared local archives and committed sources."""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess

from release import ARCHIVE, file_digest, require, verify_archive


def gh_json(*args):
    return json.loads(subprocess.check_output(["gh", *args]))


def verify_sources(files, entries):
    source_count = 0
    for name, data in files.items():
        if name in entries:
            actual = hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()
            require(actual == entries[name], f"source/package differs: {name}")
            source_count += 1
    for name in entries:
        if name.startswith(("crates/avm-contract-sdk/", "examples/", "profiles/")):
            require(name in files, f"source missing from package: {name}")
    return source_count


def verify_local(tag, destination, expected_sdk, expected_source, expected_archive,
                 archive, checksum, sdk):
    require(file_digest(archive) == expected_archive,
            "local archive differs from the independently qualified archive")
    files = verify_archive(archive, checksum, tag)
    manifest = json.loads(files["BUILD-MANIFEST.json"])
    require(manifest["sdk_commit"] == expected_sdk and manifest["ackinacki_commit"] == expected_source,
            "local full commits differ from reviewed heads")
    # Read the exact commit, never dirty working files or a mutable branch name.
    commit = subprocess.check_output(
        ["git", "-C", str(sdk), "rev-parse", "--verify", expected_sdk + "^{commit}"], text=True).strip()
    require(commit == expected_sdk, "local SDK commit differs from reviewed head")
    tree = subprocess.check_output(["git", "-C", str(sdk), "ls-tree", "-r", "-z", expected_sdk])
    entries = {}
    for record in tree.split(b"\0"):
        if record:
            metadata, name = record.split(b"\t", 1)
            _, kind, identity = metadata.split()
            if kind == b"blob":
                entries[name.decode()] = identity.decode()
    count = verify_sources(files, entries)
    result = {"verified": True, "mode": "local", "tag": tag,
              "sdk_commit": expected_sdk, "source_commit": expected_source,
              "archive_sha256": file_digest(archive), "source_files_verified": count,
              "binaries": json.loads(files["provenance/build.json"])["binaries"]}
    destination.mkdir(parents=True, exist_ok=False)
    (destination / "local-verification.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))


def verify(tag, destination, expected_sdk, expected_source, expected_archive):
    repository = "gosh-sh/avm-sdk"
    release = gh_json("release", "view", tag, "--repo", repository,
                      "--json", "tagName,isDraft,isPrerelease,url,assets")
    require(release["tagName"] == tag and not release["isDraft"] and not release["isPrerelease"], "release is not the requested final public release")
    names = [asset["name"] for asset in release["assets"]]
    require(names.count(ARCHIVE) == 1 and names.count(ARCHIVE + ".sha256") == 1, "missing or ambiguous public assets")
    destination.mkdir(parents=True, exist_ok=False)
    subprocess.run(["gh", "release", "download", tag, "--repo", repository, "--pattern", ARCHIVE,
                    "--pattern", ARCHIVE + ".sha256", "--dir", str(destination)], check=True)
    archive = destination / ARCHIVE
    require(file_digest(archive) == expected_archive, "download differs from the independently qualified archive")
    files = verify_archive(archive, destination / (ARCHIVE + ".sha256"), tag)
    manifest = json.loads(files["BUILD-MANIFEST.json"])
    require(manifest["sdk_commit"] == expected_sdk and manifest["ackinacki_commit"] == expected_source, "published full commits differ from reviewed heads")
    ref = gh_json("api", f"repos/{repository}/git/ref/tags/{tag}")["object"]
    while ref["type"] == "tag":
        ref = gh_json("api", f"repos/{repository}/git/tags/{ref['sha']}")["object"]
    require(ref["type"] == "commit" and ref["sha"] == expected_sdk, "public tag does not point to the exact SDK commit")
    tree = gh_json("api", f"repos/{repository}/git/trees/{expected_sdk}?recursive=1")
    require(not tree["truncated"], "public SDK tree is incomplete")
    entries = {entry["path"]: entry["sha"] for entry in tree["tree"] if entry["type"] == "blob"}
    source_count = verify_sources(files, entries)
    result = {"verified": True, "release_url": release["url"], "tag": tag,
              "sdk_commit": expected_sdk, "source_commit": expected_source,
              "archive_sha256": file_digest(archive), "source_files_verified": source_count,
              "binaries": json.loads(files["provenance/build.json"])["binaries"]}
    (destination / "public-verification.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--sdk-commit", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--archive-sha256", required=True)
    parser.add_argument("--archive", type=Path, help="local mode: prepared archive; no GitHub requests")
    parser.add_argument("--checksum", type=Path, help="local mode: prepared archive checksum")
    parser.add_argument("--sdk", type=Path, help="local mode: repository containing the exact SDK commit")
    args = parser.parse_args()
    if any((args.archive, args.checksum, args.sdk)):
        if not all((args.archive, args.checksum, args.sdk)):
            parser.error("local mode requires --archive, --checksum and --sdk")
        verify_local(args.tag, args.destination.resolve(), args.sdk_commit, args.source_commit,
                     args.archive_sha256, args.archive.resolve(), args.checksum.resolve(), args.sdk.resolve())
    else:
        verify(args.tag, args.destination.resolve(), args.sdk_commit, args.source_commit, args.archive_sha256)
