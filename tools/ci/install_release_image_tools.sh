#!/usr/bin/env bash
set -euo pipefail

task_dir="${1:?usage: install_release_image_tools.sh TASK_DIR}"
repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
policy="${repo_root}/.github/security/release-image-policy.json"
mkdir -p "${task_dir}/tools" "${task_dir}/evidence"
version="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["trivy_version"])' "${policy}")"
checksum="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["trivy_linux_amd64_sha256"])' "${policy}")"
archive="${task_dir}/tools/trivy.tar.gz"
curl --fail --show-error --silent --location --retry 3 --max-time 180 \
  "https://github.com/aquasecurity/trivy/releases/download/v${version}/trivy_${version}_Linux-64bit.tar.gz" \
  --output "${archive}"
printf '%s  %s\n' "${checksum}" "${archive}" | sha256sum --check --strict
tar -xzf "${archive}" -C "${task_dir}/tools" trivy
chmod 0755 "${task_dir}/tools/trivy"
"${task_dir}/tools/trivy" --version > "${task_dir}/evidence/scanner-version.txt"
