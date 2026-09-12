# Issue #220: dependency update coverage

Revalidated against fork main `f11d9ff3bc27aee42e54982cbe24343c1627ab5e`.
The original installer checksum and required advisory-gate defects have merged
fixes. A separate maintenance gap remains: Dependabot config covers only two
of the five npm projects with tracked manifests and lockfiles and has no Docker updater.

## Decision

Add all three VSCodex npm projects without overlapping
the existing frontend and automation entries. Add the three directories holding
tracked Dockerfiles: root, VSCodex and the root-logging fixture under `tests`.
Dependabot's Docker fetcher matches `dockerfile|containerfile` case-insensitively,
so `Dockerfile.app` variants and `root_logging.Dockerfile` are included.
Source: [Dependabot Docker fetcher](https://github.com/dependabot/dependabot-core/blob/main/docker/lib/dependabot/docker/file_fetcher.rb).

Independent review identified that the root `package-lock.json` has no tracked
`package.json` and contains no dependencies. It is an orphan lockfile, not a sixth
npm project. Remove the initially proposed root npm target because Dependabot's
npm fetcher requires `package.json`; leave the orphan file unchanged. Count npm
coverage using tracked manifest/lockfile pairs, not lockfiles alone.
Source: [Dependabot npm fetcher](https://github.com/dependabot/dependabot-core/blob/main/npm_and_yarn/lib/dependabot/npm_and_yarn/file_fetcher.rb).

Reuse the existing weekly cadence, five-open-PR limit and minor/patch grouping
for the new update entries. Major updates remain individually reviewable.
Version updates still require the repository's normal review and CI process.
This config does not enable a new automatic merge path or update dependencies
in this PR. Changes apply only to the fork repository.

Benefit: routine version updates cover every committed npm project and Docker
build directory. Complexity: S; risk: low, limited to update proposal scheduling.
The existing Cargo/npm vulnerability checks remain independent merge gates.

## Verification and completion boundary

YAML parsing and a comparison with the tracked manifest/lockfile pairs and Dockerfile inventory
passed: all five npm project directories and all three Dockerfile directories have
exactly one updater. The baseline was missing three npm and three Docker
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
