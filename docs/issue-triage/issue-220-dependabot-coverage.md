# Issue #220: dependency update coverage

Revalidated against fork main `f11d9ff3bc27aee42e54982cbe24343c1627ab5e`.
The original installer checksum and required advisory-gate defects have merged
fixes. A separate maintenance gap remains: Dependabot config covers only two
of the six committed npm lockfile directories and has no Docker updater.

## Decision

Add the root npm project and all three VSCodex npm projects without overlapping
the existing frontend and automation entries. Add the three directories holding
tracked Dockerfiles: root, VSCodex and the root-logging fixture under `tests`.
Dependabot's Docker fetcher matches `dockerfile|containerfile` case-insensitively,
so `Dockerfile.app` variants and `root_logging.Dockerfile` are included.
Source: [Dependabot Docker fetcher](https://github.com/dependabot/dependabot-core/blob/main/docker/lib/dependabot/docker/file_fetcher.rb).

Reuse the existing weekly cadence, five-open-PR limit and minor/patch grouping
for the new update entries. Major updates remain individually reviewable.
Version updates still require the repository's normal review and CI process.
This config does not enable a new automatic merge path or update dependencies
in this PR. Changes apply only to the fork repository.

Benefit: routine version updates cover every committed npm project and Docker
build directory. Complexity: S; risk: low, limited to update proposal scheduling.
The existing Cargo/npm vulnerability checks remain independent merge gates.

## Verification and completion boundary

YAML parsing and a comparison with the tracked lockfile/Dockerfile inventory
passed: all six npm directories and all three Dockerfile directories have
exactly one updater. The baseline was missing four npm and three Docker
directories. All four existing update entries are preserved; every entry uses
weekly scheduling and a positive PR limit no greater than five. The patch
passed `git diff --check`. The actual hosted update job's first successful run remains
operational evidence to collect after merge; configuration coverage alone does
not prove every image reference is resolvable by the hosted service.

This does not pin local build-image digests, change container UID or volume
permissions, resolve the tracked RSA exception, or produce vulnerability-scan
artifacts. Those #220/#303 acceptance items retain their separate scope.
Keep #220 open after this slice. Rollback is a revert of the new updater entries;
already-open Dependabot proposals remain reviewable through normal PR history.
