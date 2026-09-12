# Gateway readiness withdrawal and recovery drill

`tools/ci/run_readiness_drill.sh` starts the real `aether-gateway` binary against
its own loopback-only PostgreSQL and Redis processes. It verifies HTTP
withdrawal and recovery without connecting to a deployed environment.

Prerequisites: Bash, Node.js, `curl`, `jq`, `pgrep`, PostgreSQL (`initdb`, `postgres`,
`pg_ctl`, `psql`), and `redis-server`. Run as an unprivileged user. Missing
executables fail the drill; they never produce a skipped success. Docker is
not required. On macOS the PostgreSQL binaries can be installed with Homebrew;
Linux CI can install PostgreSQL and Redis packages.

Build and execute from the repository root:

```sh
cargo build --locked -p aether-gateway --bin aether-gateway
AETHER_GATEWAY_BIN="$PWD/target/debug/aether-gateway" \
  bash tools/ci/run_readiness_drill.sh
```

Set `AETHER_GATEWAY_BIN` to the actual artifact when using a different Cargo
target directory. Optional executable overrides are `AETHER_INITDB_BIN`,
`AETHER_POSTGRES_BIN`, `AETHER_PG_CTL_BIN`, `AETHER_PSQL_BIN`, and
`AETHER_TEST_REDIS_SERVER_BIN`. `AETHER_READINESS_EVIDENCE_DIR` can name a new
directory; the script refuses to overwrite an existing evidence directory.

The fixture uses three independently allocated loopback ports, a new temporary
database cluster, non-persistent Redis storage, and disposable test-only
credentials. The gateway receives a clean environment and runs from the fixture
directory, so inherited production connection strings, proxy settings, and
repository environment files do not select its dependencies. The script accepts
no external database or Redis URL. It stops only processes that it started and
removes only its own temporary fixture directory, including when interrupted.
Evidence is retained separately.

The gateway currently binds its ephemeral HTTP port on all interfaces. Run this
disposable drill on a development/CI host with the usual network isolation; the
PostgreSQL and Redis fixtures themselves listen only on loopback.

| Phase | `/readyz` | Database | Redis | `/health` |
| --- | --- | --- | --- | --- |
| Initial startup completed | 200 | `ok` | `ok` | 200 |
| PostgreSQL stopped | 503 | `failed` or `timeout` | `ok` | 200 |
| PostgreSQL restarted | 200 | `ok` | `ok` | 200 |
| PostgreSQL master and backends suspended | 503 | `timeout` | `ok` | 200 |
| PostgreSQL resumed | 200 | `ok` | `ok` | 200 |
| Redis stopped | 503 | `ok` | `failed` or `timeout` | 200 |
| Redis restarted | 200 | `ok` | `ok` | 200 |
| Redis process suspended | 503 | `ok` | `timeout` | 200 |
| Redis resumed | 200 | `ok` | `ok` | 200 |
| Both dependencies suspended | 503 | `timeout` | `timeout` | 200 |
| Both dependencies resumed | 200 | `ok` | `ok` | 200 |
| SIGTERM withdrawal window | 503, `closing` | `unchecked` | `unchecked` | 200 |

Each response must retain the documented ready/not-ready envelope and stable
dependency status objects. The drill rejects leaked URLs, credentials, or raw
connection details. The deadline permits the readiness cache and pooled
connections to observe a state transition; every HTTP observation has a two
second client bound, and completed readiness responses must finish within
1.5 seconds including scheduling/network overhead. During withdrawal/recovery polling, the drill checks
liveness after each readiness observation. Suspended dependency processes exercise
the shared probe deadline independently from connection failures. The final
SIGTERM check observes `closing` while the real listener still answers liveness,
then requires clean exit within the configured withdrawal and drain budget.
The fixture uses a three-second withdrawal window to make this observation
reproducible; the production default is two seconds and can be adjusted for the
deployment's probe interval and load-balancer propagation time.

The final output prints the evidence directory. Retain `result.log`,
`http-observations.tsv`, per-attempt JSON/headers/timings, `gateway.log`,
PostgreSQL/Redis logs, executable versions and gateway SHA-256, and `cleanup.log` as the acceptance
artifact. No production secrets are needed to reproduce it.

This drill demonstrates local dependency-sensitive readiness and liveness
independence. It does not claim a particular orchestrator has removed the node
from production traffic, nor does it validate a real multi-node deployment.
Configure the orchestrator's readiness check for `/readyz` and liveness check
for `/health`; preserve a startup allowance for database preparation before
interpreting probe failures.
