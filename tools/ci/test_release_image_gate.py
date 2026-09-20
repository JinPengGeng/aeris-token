#!/usr/bin/env python3
"""Executable release-boundary fixtures; no Docker, registry or scanner required."""

import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest


SCRIPT = Path(__file__).with_name("release_image_gate.py")
SPEC = importlib.util.spec_from_file_location("image_gate", SCRIPT)
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)


def image_archive(path, platforms=("amd64", "arm64"), corrupt=False, mismatch=False):
    blobs = {}
    def add(data, media):
        raw = data if isinstance(data, bytes) else json.dumps(data).encode()
        value = GATE.digest(raw)
        blobs["blobs/sha256/" + value.split(":")[1]] = raw
        return {"mediaType": media, "size": len(raw), "digest": value}
    layer_stream = io.BytesIO()
    with tarfile.open(fileobj=layer_stream, mode="w") as layer:
        for name, content in {
            "etc/debian_version": "12.12\n",
            "etc/os-release": 'ID=debian\nVERSION_ID="12"\nPRETTY_NAME="Debian GNU/Linux 12"\n',
            "var/lib/dpkg/status": "Package: base-files\nStatus: install ok installed\nArchitecture: amd64\nVersion: 12.4+deb12u12\n\n",
        }.items():
            entry = tarfile.TarInfo(name)
            entry.size = len(content.encode())
            layer.addfile(entry, io.BytesIO(content.encode()))
    layer_bytes = layer_stream.getvalue()
    layer = add(layer_bytes, "application/vnd.oci.image.layer.v1.tar")
    manifests = []
    for arch in platforms:
        config = add({"os": "linux", "architecture": "amd64" if mismatch else arch,
                      "rootfs": {"type": "layers", "diff_ids": [GATE.digest(layer_bytes)]}},
                     "application/vnd.oci.image.config.v1+json")
        manifest = add({"schemaVersion": 2, "mediaType": GATE.MANIFEST,
                        "config": config, "layers": [layer]}, GATE.MANIFEST)
        manifest["platform"] = {"os": "linux", "architecture": arch}
        manifests.append(manifest)
    root = add({"schemaVersion": 2, "mediaType": GATE.INDEX, "manifests": manifests}, GATE.INDEX)
    blobs["index.json"] = json.dumps({"schemaVersion": 2, "manifests": [root]}).encode()
    blobs["oci-layout"] = b'{"imageLayoutVersion":"1.0.0"}'
    if corrupt:
        blobs[next(iter(blobs))] += b"corrupted"
    with tarfile.open(path, "w") as output:
        for name, data in blobs.items():
            entry = tarfile.TarInfo(name)
            entry.size = len(data)
            output.addfile(entry, io.BytesIO(data))
    return root["digest"]


class GateFixtures(unittest.TestCase):
    def setUp(self):
        temporary = Path(os.environ.get("AGENT_TMP_DIR", str(Path.home() / ".agents/tmp")))
        temporary.mkdir(parents=True, exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(prefix="release-image-fixture-", dir=temporary)
        self.root = Path(self.temp.name)
        self.archive = self.root / "release.oci.tar"
        self.evidence = self.root / "evidence"
        self.expected = image_archive(self.archive)
        self.tags = self.root / "tags"
        self.tags.write_text("ghcr.io/example/aether:1.0.0\nghcr.io/example/aether:latest\n")
        self.scanner = self.root / "trivy"
        self.publisher = self.root / "skopeo"
        self.install_scanner()
        self.publisher.write_text(f'''#!{sys.executable}
import pathlib,sys,tarfile
root=pathlib.Path(__file__).parent
(root/"publisher-called").touch()
if sys.argv[1]=="inspect":
 with tarfile.open(root/"release.oci.tar") as archive:
  sys.stdout.buffer.write(archive.extractfile("blobs/sha256/"+{self.expected!r}.split(":")[1]).read())
 sys.exit(0)
pathlib.Path(sys.argv[sys.argv.index("--digestfile")+1]).write_text({self.expected!r})
''')
        self.publisher.chmod(0o755)

    def tearDown(self):
        self.temp.cleanup()

    def install_scanner(self, mode="ok"):
        self.scanner.write_text(f'''#!{sys.executable}
import datetime as dt,json,pathlib,sys
mode={mode!r}
args=sys.argv
if "--version" in args:
 print("Version: 0.74.0");sys.exit(0)
if "--download-db-only" in args:
 now=dt.datetime.now(dt.timezone.utc)
 old=dt.timedelta(days=4) if mode=="stale-db" else dt.timedelta(seconds=0)
 cache=pathlib.Path(args[args.index("--cache-dir")+1])/"db"
 cache.mkdir(parents=True)
 (cache/"metadata.json").write_text(json.dumps({{"Version":2,"UpdatedAt":(now-old).isoformat(),"NextUpdate":(now+dt.timedelta(hours=1)-old).isoformat()}}))
 sys.exit(0)
layout=pathlib.Path(args[args.index("--input")+1].split("@")[0])
index=json.loads((layout/"index.json").read_text())
assert len(index["manifests"])==1
descriptor=index["manifests"][0]
assert args[args.index("--input")+1].endswith("@"+descriptor["digest"])
manifest=json.loads((layout/"blobs/sha256"/descriptor["digest"].split(":")[1]).read_text())
config=json.loads((layout/"blobs/sha256"/manifest["config"]["digest"].split(":")[1]).read_text())
if mode=="scanner-error" and config["architecture"]=="arm64": sys.exit(2)
if mode=="wrong-arch": config["architecture"]="amd64"
bad=mode=="arm64-vulnerable" and config["architecture"]=="arm64"
report={{"SchemaVersion":2,"ArtifactType":"container_image","Metadata":{{"ImageID":manifest["config"]["digest"],"ImageConfig":config,"OS":{{"Family":"debian"}}}},"Results":[{{"Class":"os-pkgs","Packages":[{{"Name":"base-files"}}],"Vulnerabilities":[{{"Severity":"HIGH"}}] if bad else []}}]}}
if mode=="no-coverage": report["Results"]=[]
pathlib.Path(args[args.index("--output")+1]).write_text(json.dumps(report))
sys.exit(1 if bad else 0)
''')
        self.scanner.chmod(0o755)

    def invoke(self, action, extra=()):
        command = [sys.executable, "-B", str(SCRIPT), action, "--archive", str(self.archive), "--evidence", str(self.evidence)]
        if action == "scan":
            command += ["--trivy", str(self.scanner), "--db-reference", "ghcr.io/aquasecurity/trivy-db@sha256:" + "a" * 64]
        else:
            command += ["--skopeo", str(self.publisher), "--tags-file", str(self.tags)]
        return subprocess.run(command + list(extra), capture_output=True, text=True, check=False)

    def assert_blocked(self):
        result = self.invoke("scan")
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertTrue((self.evidence / "scan-failure.json").is_file())
        self.assertFalse((self.evidence / "verified.json").exists())
        self.assertNotEqual(self.invoke("publish").returncode, 0)
        self.assertFalse((self.root / "publisher-called").exists())

    def test_verified_two_platform_archive_publishes_same_index(self):
        result = self.invoke("scan")
        self.assertEqual(result.returncode, 0, result.stderr)
        inventory = GATE.read_json(self.evidence / "inventory.json")
        self.assertEqual(set(inventory["images"]), {"linux/amd64", "linux/arm64"})
        self.assertNotEqual(inventory["images"]["linux/amd64"]["config_digest"], inventory["images"]["linux/arm64"]["config_digest"])
        result = self.invoke("publish")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(GATE.read_json(self.evidence / "published.json")["digest"], self.expected)

    def test_only_arm64_vulnerable_blocks_and_keeps_both_reports(self):
        self.install_scanner("arm64-vulnerable")
        self.assert_blocked()
        self.assertTrue((self.evidence / "amd64.json").is_file())
        self.assertTrue((self.evidence / "arm64.json").is_file())

    def test_missing_architecture_blocks(self):
        image_archive(self.archive, platforms=("amd64",))
        self.assert_blocked()

    def test_duplicate_architecture_blocks(self):
        image_archive(self.archive, platforms=("amd64", "amd64", "arm64"))
        self.assert_blocked()

    def test_corrupt_blob_blocks(self):
        image_archive(self.archive, corrupt=True)
        self.assert_blocked()

    def test_descriptor_config_mismatch_blocks(self):
        image_archive(self.archive, mismatch=True)
        self.assert_blocked()

    def test_expired_database_blocks(self):
        self.install_scanner("stale-db")
        self.assert_blocked()

    def test_scanner_error_blocks(self):
        self.install_scanner("scanner-error")
        self.assert_blocked()
        self.assertTrue((self.evidence / "arm64.log").is_file())

    def test_false_double_amd64_scan_blocks(self):
        self.install_scanner("wrong-arch")
        self.assert_blocked()

    def test_empty_scan_inventory_blocks(self):
        self.install_scanner("no-coverage")
        self.assert_blocked()

    def test_changed_archive_blocks_before_copy(self):
        self.assertEqual(self.invoke("scan").returncode, 0)
        with self.archive.open("ab") as stream:
            stream.write(b"changed")
        self.assertNotEqual(self.invoke("publish").returncode, 0)
        self.assertFalse((self.root / "publisher-called").exists())

    def test_buildx_digest_mismatch_blocks(self):
        result = self.invoke("scan", ("--expected-index-digest", "sha256:" + "c" * 64))
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.evidence / "verified.json").exists())

    def test_changed_report_blocks_before_copy(self):
        self.assertEqual(self.invoke("scan").returncode, 0)
        (self.evidence / "arm64.json").write_text("{}")
        self.assertNotEqual(self.invoke("publish").returncode, 0)
        self.assertFalse((self.root / "publisher-called").exists())

    def test_registry_tag_digest_mismatch_blocks(self):
        self.assertEqual(self.invoke("scan").returncode, 0)
        content = self.publisher.read_text()
        content = content.replace('sys.stdout.buffer.write(archive.extractfile(', 'sys.stdout.buffer.write(b"changed"+archive.extractfile(')
        self.publisher.write_text(content)
        self.assertNotEqual(self.invoke("publish").returncode, 0)
        self.assertFalse((self.evidence / "published.json").exists())

    def test_published_digest_mismatch_blocks_attestation_output(self):
        self.assertEqual(self.invoke("scan").returncode, 0)
        self.publisher.write_text(self.publisher.read_text().replace(self.expected, "sha256:" + "b" * 64))
        self.assertNotEqual(self.invoke("publish").returncode, 0)
        self.assertFalse((self.evidence / "published.json").exists())
        self.assertEqual(len(list(self.evidence.glob("push-*.log"))), 1)


if __name__ == "__main__":
    unittest.main()
