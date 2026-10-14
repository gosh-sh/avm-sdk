#!/usr/bin/env python3
"""Read-only post-publication verification against actual public GitHub assets."""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess

from release import ARCHIVE, file_digest, require, verify_archive


def gh_json(*args):
    return json.loads(subprocess.check_output(["gh", *args]))


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
    entries = {entry["path"]: entry for entry in tree["tree"] if entry["type"] == "blob"}
    source_count = 0
    for name, data in files.items():
        if name in entries:
            actual = hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()
            require(actual == entries[name]["sha"], f"public source/package differs: {name}")
            source_count += 1
    for name in entries:
        if name.startswith(("crates/avm-contract-sdk/", "examples/", "profiles/")):
            require(name in files, f"public source missing from package: {name}")
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
    args = parser.parse_args()
    verify(args.tag, args.destination.resolve(), args.sdk_commit, args.source_commit, args.archive_sha256)
