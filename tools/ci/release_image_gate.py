#!/usr/bin/env python3
"""Scan each verified OCI child before publishing the unchanged multiarch archive.

No registry writes occur in scan. Local layouts are derived views of the original
blobs; Trivy's OCI reader otherwise selects the first child regardless of platform.
"""

import argparse
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import urllib.request


ROOT = Path(__file__).resolve().parents[2]
POLICY = ROOT / ".github/security/release-image-policy.json"
INDEX = "application/vnd.oci.image.index.v1+json"
MANIFEST = "application/vnd.oci.image.manifest.v1+json"
DIGEST = re.compile(r"sha256:[0-9a-f]{64}\Z")


def require(condition, message):
    if not condition:
        raise ValueError(message)


def digest(data):
    return "sha256:" + hashlib.sha256(data).hexdigest()


def file_digest(path):
    h = hashlib.sha256()
    with open(path, "rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(chunk)
    return "sha256:" + h.hexdigest()


def read_json(path):
    return json.loads(Path(path).read_text())


def write_json(path, data):
    Path(path).write_text(json.dumps(data, indent=2, sort_keys=True) + "\n")


def load_policy():
    policy = read_json(POLICY)
    require(policy["schema_version"] == 1, "unsupported image policy")
    require(policy["platforms"] == ["linux/amd64", "linux/arm64"], "unexpected platform policy")
    require(policy["blocked_severities"] == ["HIGH", "CRITICAL"], "unexpected severity policy")
    return policy


def unpack_archive(archive, destination):
    destination.mkdir()
    seen = set()
    with tarfile.open(archive, "r:*") as source:
        for member in source:
            name = member.name.removeprefix("./")
            require(name not in seen, "duplicate archive member")
            seen.add(name)
            if member.isdir():
                require(name.rstrip("/") in ("", ".", "blobs", "blobs/sha256"), "unexpected OCI directory")
                continue
            require(member.isfile(), "OCI archive links and special files are forbidden")
            require(name in ("index.json", "oci-layout") or re.fullmatch(r"blobs/sha256/[0-9a-f]{64}", name), "unexpected OCI member")
            target = destination / name
            target.parent.mkdir(parents=True, exist_ok=True)
            with source.extractfile(member) as src, target.open("wb") as dst:
                shutil.copyfileobj(src, dst)
            if name.startswith("blobs/"):
                require(file_digest(target) == "sha256:" + target.name, "OCI blob hash mismatch")
    require(read_json(destination / "oci-layout")["imageLayoutVersion"] == "1.0.0", "unsupported OCI layout")


def blob(layout, descriptor, parse=False):
    value = descriptor["digest"]
    require(DIGEST.fullmatch(value), "invalid OCI digest")
    path = layout / "blobs" / "sha256" / value.split(":")[1]
    require(path.stat().st_size == descriptor["size"], "OCI descriptor size mismatch")
    require(file_digest(path) == value, "OCI descriptor hash mismatch")
    return read_json(path) if parse else path


def prepare(archive, work, policy):
    layout = work / "original"
    unpack_archive(archive, layout)
    outer = read_json(layout / "index.json")
    require(outer.get("schemaVersion") == 2 and outer.get("manifests"), "invalid OCI root index")
    # Buildx exports an OCI layout envelope whose descriptors name the publishable
    # index. Multiple tag annotations may name the same immutable index.
    roots = outer["manifests"]
    require(len({item["digest"] for item in roots}) == 1, "ambiguous publishable OCI root")
    require(all(item.get("mediaType") == INDEX for item in roots), "expected multiarch index descriptor")
    for item in roots:
        blob(layout, item)
    root = roots[0]
    index = blob(layout, root, True)
    require(index.get("schemaVersion") == 2 and index.get("mediaType") == INDEX, "invalid publishable OCI index")
    images = {}
    attestations = []
    for descriptor in index["manifests"]:
        require(descriptor.get("mediaType") == MANIFEST, "unexpected nested OCI index/artifact")
        manifest = blob(layout, descriptor, True)
        require(manifest.get("schemaVersion") == 2 and manifest.get("mediaType") == MANIFEST, "invalid child manifest")
        config = blob(layout, manifest["config"], True)
        for layer in manifest["layers"]:
            blob(layout, layer)
        platform = descriptor.get("platform", {})
        key = platform.get("os", "") + "/" + platform.get("architecture", "")
        if key == "unknown/unknown":
            annotations = descriptor.get("annotations", {})
            require(annotations.get("vnd.docker.reference.type") == "attestation-manifest", "unclassified OCI artifact")
            attestations.append(annotations.get("vnd.docker.reference.digest"))
            continue
        require(key in policy["platforms"] and key not in images, "missing/duplicate/unexpected runtime platform")
        require(config.get("os") == platform["os"] and config.get("architecture") == platform["architecture"], "descriptor/config architecture mismatch")
        require(not platform.get("variant") or (key == "linux/arm64" and platform["variant"] == "v8"), "unexpected platform variant")
        child = work / ("scan-" + platform["architecture"])
        child.mkdir()
        shutil.copyfile(layout / "oci-layout", child / "oci-layout")
        # A symlink to verified local blobs saves disk. The untrusted input archive
        # itself cannot contain symlinks. Never mutate the original index.
        (child / "blobs").symlink_to(layout / "blobs", target_is_directory=True)
        write_json(child / "index.json", {"schemaVersion": 2, "mediaType": INDEX, "manifests": [descriptor]})
        images[key] = {"manifest_digest": descriptor["digest"], "config_digest": manifest["config"]["digest"], "layout": str(child)}
    require(set(images) == set(policy["platforms"]), "missing runtime architecture")
    require(all(item in {x["manifest_digest"] for x in images.values()} for item in attestations), "orphan OCI attestation")
    return {"archive_digest": file_digest(archive), "index_digest": root["digest"], "images": images}


def resolve_db(policy):
    require(policy["db_repository"] == "ghcr.io/aquasecurity/trivy-db:2", "DB must resolve from the official repository")
    token_url = "https://ghcr.io/token?service=ghcr.io&scope=repository:aquasecurity/trivy-db:pull"
    with urllib.request.urlopen(token_url, timeout=60) as response:
        token = json.load(response)["token"]
    request = urllib.request.Request("https://ghcr.io/v2/aquasecurity/trivy-db/manifests/2", headers={
        "Authorization": "Bearer " + token, "Accept": MANIFEST,
    })
    with urllib.request.urlopen(request, timeout=60) as response:
        data = response.read()
        resolved = digest(data)
        require(response.headers.get("Docker-Content-Digest") == resolved, "DB registry digest mismatch")
    require(json.loads(data).get("mediaType") == MANIFEST, "unexpected DB manifest")
    return "ghcr.io/aquasecurity/trivy-db@" + resolved


def validate_db(metadata, policy):
    now = dt.datetime.now(dt.timezone.utc)
    def timestamp(field):
        value = dt.datetime.fromisoformat(metadata[field].replace("Z", "+00:00"))
        require(value.tzinfo is not None, "DB timestamp has no timezone")
        return value
    updated, next_update = timestamp("UpdatedAt"), timestamp("NextUpdate")
    require(metadata["Version"] == 2, "unsupported DB schema")
    require(updated <= now + dt.timedelta(seconds=policy["clock_skew_seconds"]), "DB timestamp is in the future")
    require(now - updated <= dt.timedelta(hours=policy["db_max_age_hours"]), "DB is too old")
    require(next_update > now and next_update > updated, "DB NextUpdate has expired")


def run_logged(command, log, stdout_path=None):
    with Path(log).open("w") as stream:
        try:
            # A runner's TRIVY_* variables must not silently weaken the policy.
            environment = {key: value for key, value in os.environ.items() if not key.startswith("TRIVY_")}
            if stdout_path is not None:
                with Path(stdout_path).open("wb") as output:
                    return subprocess.run(command, stdout=output, stderr=stream, timeout=900,
                                          check=False, env=environment).returncode
            return subprocess.run(command, stdout=stream, stderr=subprocess.STDOUT,
                                  timeout=900, check=False, env=environment).returncode
        except (OSError, subprocess.TimeoutExpired) as error:
            stream.write(str(error) + "\n")
            return 125


def validate_report(report, image, platform, policy):
    require(report.get("SchemaVersion") == 2 and report.get("ArtifactType") == "container_image", "missing container scan coverage")
    metadata = report["Metadata"]
    require(metadata.get("ImageID") == image["config_digest"], "scanner covered the wrong config digest")
    require(metadata["ImageConfig"].get("os") + "/" + metadata["ImageConfig"].get("architecture") == platform, "scanner covered the wrong architecture")
    require(metadata.get("OS", {}).get("Family") == policy["expected_os_family"], "scanner did not identify the runtime OS")
    results = report.get("Results", [])
    require(any(item.get("Class") == "os-pkgs" and item.get("Packages") for item in results), "scanner produced no OS package inventory")
    for item in results:
        for vulnerability in item.get("Vulnerabilities") or []:
            require(vulnerability["Severity"] not in policy["blocked_severities"], "image has HIGH/CRITICAL vulnerabilities")


def scan(args, policy):
    evidence = args.evidence
    verdict = evidence / "verified.json"
    verdict.unlink(missing_ok=True)
    work = evidence.parent / "scan-work"
    work.mkdir()  # Refuse cache/layout reuse between releases.
    inventory = prepare(args.archive, work, policy)
    write_json(evidence / "inventory.json", inventory)
    if args.expected_index_digest is not None:
        require(inventory["index_digest"] == args.expected_index_digest, "archive index differs from Buildx output")
    shutil.copyfile(work / "original/blobs/sha256" / inventory["index_digest"].split(":")[1], evidence / "oci-index.json")
    shutil.copyfile(POLICY, evidence / "policy.json")
    db_ref = args.db_reference or resolve_db(policy)
    require(re.fullmatch(r"ghcr\.io/aquasecurity/trivy-db@sha256:[0-9a-f]{64}", db_ref), "DB is not pinned to the official digest")
    write_json(evidence / "database.json", {"reference": db_ref, "resolved_at": dt.datetime.now(dt.timezone.utc).isoformat()})
    cache = work / "cache"
    config = work / "trivy.yaml"
    config.write_text("{}\n")
    ignore = work / "ignore"
    ignore.write_text("")
    base = [str(args.trivy), "image", "--config", str(config), "--cache-dir", str(cache), "--db-repository", db_ref]
    version = subprocess.check_output([str(args.trivy), "--version"], text=True, timeout=30)
    (evidence / "scanner-version.txt").write_text(version)
    require("Version: " + policy["trivy_version"] + "\n" in version, "unexpected scanner version")
    require(run_logged(base + ["--download-db-only", "--no-progress"], evidence / "database-download.log") == 0, "DB download failed")
    metadata = read_json(cache / "db/metadata.json")
    write_json(evidence / "database-metadata.json", metadata)
    validate_db(metadata, policy)
    failures = []
    reports = {}
    for platform, image in inventory["images"].items():
        arch = platform.split("/")[1]
        report_path = evidence / (arch + ".json")
        command = base + ["--input", image["layout"] + "@" + image["manifest_digest"],
                          "--skip-db-update", "--skip-java-db-update", "--skip-version-check",
                          "--offline-scan", "--scanners", "vuln",
                          "--pkg-types", "os,library",
                          "--severity", ",".join(policy["blocked_severities"]), "--ignore-unfixed=false",
                          "--ignorefile", str(ignore), "--list-all-pkgs", "--format", "json",
                          "--output", str(report_path), "--exit-code", "1", "--no-progress"]
        code = run_logged(command, evidence / (arch + ".log"))
        try:
            require(code == 0, f"scanner exited {code}")
            validate_report(read_json(report_path), image, platform, policy)
            reports[arch + ".json"] = file_digest(report_path)
        except (ValueError, KeyError, TypeError, OSError) as error:
            failures.append(platform + ": " + str(error))
    require(not failures, "; ".join(failures))
    validate_db(metadata, policy)
    write_json(verdict, {**inventory, "policy_digest": file_digest(POLICY), "reports": reports,
                         "database_digest": file_digest(evidence / "database-metadata.json"), "db_reference": db_ref})


def publish(args, policy):
    evidence = args.evidence
    (evidence / "published.json").unlink(missing_ok=True)
    verified = read_json(evidence / "verified.json")
    require(verified["policy_digest"] == file_digest(POLICY), "policy changed after scanning")
    require(verified["archive_digest"] == file_digest(args.archive), "archive changed after scanning")
    require(set(verified["images"]) == set(policy["platforms"]), "scan missing architecture")
    require(set(verified["reports"]) == {"amd64.json", "arm64.json"}, "scan missing report")
    for name, expected in verified["reports"].items():
        require(file_digest(evidence / name) == expected, "scan report changed")
        arch = name.removesuffix(".json")
        validate_report(read_json(evidence / name), verified["images"]["linux/" + arch], "linux/" + arch, policy)
    require(file_digest(evidence / "database-metadata.json") == verified["database_digest"], "DB metadata changed")
    validate_db(read_json(evidence / "database-metadata.json"), policy)
    tags = list(dict.fromkeys(args.tags_file.read_text().splitlines()))
    require(tags and all(re.fullmatch(r"(?:ghcr\.io|docker\.io)/[a-z0-9][a-z0-9._/-]*:[A-Za-z0-9_][A-Za-z0-9_.-]*", tag) for tag in tags), "invalid publication tags")
    if args.skopeo:
        command = [str(args.skopeo)]
    else:
        auth = Path.home() / ".docker/config.json"
        require(auth.is_file(), "missing Docker registry auth file")
        task = evidence.parent
        command = ["docker", "run", "--rm", "--user", f"{os.getuid()}:{os.getgid()}",
                   "--volume", f"{task}:{task}", "--volume", f"{auth}:/auth/config.json:ro",
                   "--env", "REGISTRY_AUTH_FILE=/auth/config.json", policy["skopeo_image"]]
    for index, tag in enumerate(tags):
        output = evidence / f"push-{index}.digest"
        output.unlink(missing_ok=True)
        code = run_logged(command + ["copy", "--all", "--preserve-digests", "--digestfile", str(output),
                                     "oci-archive:" + str(args.archive), "docker://" + tag], evidence / f"push-{index}.log")
        require(code == 0, "registry copy failed for " + tag)
        require(output.read_text().strip() == verified["index_digest"], "published index digest mismatch")
        remote = evidence / f"push-{index}.remote-index.json"
        code = run_logged(command + ["inspect", "--raw", "docker://" + tag],
                          evidence / f"push-{index}.inspect.log", stdout_path=remote)
        require(code == 0 and file_digest(remote) == verified["index_digest"],
                "registry tag does not resolve to the scanned index")
    write_json(evidence / "published.json", {"tags": tags, "digest": verified["index_digest"]})
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a") as stream:
            stream.write("digest=" + verified["index_digest"] + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["scan", "publish"])
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    parser.add_argument("--trivy", type=Path)
    parser.add_argument("--db-reference", help="Optional pre-resolved official digest for offline validation")
    parser.add_argument("--expected-index-digest", help="Buildx digest bound to the original archive")
    parser.add_argument("--tags-file", type=Path)
    parser.add_argument("--skopeo", type=Path, help="Optional local binary; CI uses the pinned container")
    args = parser.parse_args()
    args.archive = args.archive.resolve()
    args.evidence = args.evidence.resolve()
    args.evidence.mkdir(parents=True, exist_ok=True)
    try:
        policy = load_policy()
        (scan if args.action == "scan" else publish)(args, policy)
    except Exception as error:
        write_json(args.evidence / (args.action + "-failure.json"), {"error": str(error)})
        print(str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
