#!/usr/bin/env bash
set -Eeuo pipefail
umask 077

# Every dependency belongs to this invocation. No caller-supplied database or
# Redis URL is accepted, and the gateway receives a clean environment.
: "${AETHER_GATEWAY_BIN:?Build aether-gateway and set AETHER_GATEWAY_BIN to that executable}"
gateway_bin="$(cd "$(dirname "$AETHER_GATEWAY_BIN")" && pwd)/$(basename "$AETHER_GATEWAY_BIN")"
[[ -x "$gateway_bin" ]] || { printf 'Gateway executable is missing: %s\n' "$gateway_bin" >&2; exit 2; }

initdb_bin="${AETHER_INITDB_BIN:-initdb}"
postgres_bin="${AETHER_POSTGRES_BIN:-postgres}"
pg_ctl_bin="${AETHER_PG_CTL_BIN:-pg_ctl}"
psql_bin="${AETHER_PSQL_BIN:-psql}"
redis_bin="${AETHER_TEST_REDIS_SERVER_BIN:-redis-server}"
for executable in "$initdb_bin" "$postgres_bin" "$pg_ctl_bin" "$psql_bin" "$redis_bin" curl jq node pgrep; do
  command -v "$executable" >/dev/null || { printf 'Required executable is missing: %s\n' "$executable" >&2; exit 2; }
done
[[ "$(id -u)" != 0 ]] || { printf 'Run as an unprivileged user; initdb refuses root.\n' >&2; exit 2; }

if [[ -n "${AETHER_READINESS_EVIDENCE_DIR:-}" ]]; then
  # Refuse to overwrite an earlier drill's evidence.
  mkdir "$AETHER_READINESS_EVIDENCE_DIR"
  evidence_dir="$(cd "$AETHER_READINESS_EVIDENCE_DIR" && pwd)"
else
  evidence_dir="$(mktemp -d "${TMPDIR:-/tmp}/aether-readiness-evidence.XXXXXX")"
fi
fixture_dir="$(mktemp -d "${TMPDIR:-/tmp}/aether-readiness-fixture.XXXXXX")"
postgres_pid=""
postgres_suspended_pids=()
redis_pid=""
gateway_pid=""

stop_child() {
  local child_pid="$1"
  [[ -n "$child_pid" ]] || return 0
  if kill -0 "$child_pid" 2>/dev/null; then
    # Resume a dependency if an assertion failed during the timeout scenario.
    kill -CONT "$child_pid" 2>/dev/null || true
    kill -TERM "$child_pid" 2>/dev/null || true
    for _ in {1..50}; do
      kill -0 "$child_pid" 2>/dev/null || break
      sleep 0.1
    done
    if kill -0 "$child_pid" 2>/dev/null; then
      kill -KILL "$child_pid" 2>/dev/null || true
    fi
  fi
  wait "$child_pid" 2>/dev/null || true
}

cleanup() {
  local result=$?
  trap - EXIT INT TERM
  stop_child "$gateway_pid"
  stop_child "$redis_pid"
  if (( ${#postgres_suspended_pids[@]} )); then
    kill -CONT "${postgres_suspended_pids[@]}" 2>/dev/null || true
  fi
  if [[ -n "$postgres_pid" ]]; then
    "$pg_ctl_bin" -D "$fixture_dir/postgres" stop -m fast -w -t 10 >>"$evidence_dir/cleanup.log" 2>&1 || true
    stop_child "$postgres_pid"
  fi
  # This path is created above by mktemp and never supplied by the caller.
  rm -rf -- "$fixture_dir"
  printf 'exit_code=%s\nowned_fixture_removed=true\n' "$result" >>"$evidence_dir/cleanup.log"
  printf 'Readiness drill evidence: %s\n' "$evidence_dir"
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# Hold all three ephemeral ports together while choosing distinct numbers.
ports="$(node -e '
const net = require("node:net");
const servers = Array.from({length: 3}, () => net.createServer());
Promise.all(servers.map(server => new Promise((resolve, reject) => {
  server.once("error", reject);
  server.listen(0, "127.0.0.1", () => resolve(server.address().port));
}))).then(ports => {
  console.log(ports.join(" "));
  servers.forEach(server => server.close());
}).catch(error => { console.error(error.message); process.exit(1); });
')"
read -r postgres_port redis_port gateway_port <<<"$ports"
database_url="postgres://aether@127.0.0.1:$postgres_port/postgres"
redis_url="redis://127.0.0.1:$redis_port/0"
base_url="http://127.0.0.1:$gateway_port"
printf 'phase\tattempt\tendpoint\tstatus\tseconds\n' >"$evidence_dir/http-observations.tsv"
printf 'gateway=%s\nstarted_at=%s\n' "$gateway_bin" "$(date -u +%FT%TZ)" >"$evidence_dir/run.txt"
node -e '
const fs = require("node:fs");
const hash = require("node:crypto").createHash("sha256");
const input = fs.createReadStream(process.argv[1]);
input.on("error", error => { console.error(error.message); process.exit(1); });
input.on("data", chunk => hash.update(chunk));
input.on("end", () => console.log("gateway_sha256=" + hash.digest("hex")));
' "$gateway_bin" >>"$evidence_dir/run.txt"
"$postgres_bin" --version >>"$evidence_dir/run.txt"
"$redis_bin" --version >>"$evidence_dir/run.txt"

"$initdb_bin" -D "$fixture_dir/postgres" -U aether --auth=trust --encoding=UTF8 --no-instructions >"$evidence_dir/initdb.log" 2>&1

start_postgres() {
  "$postgres_bin" -D "$fixture_dir/postgres" -h 127.0.0.1 -p "$postgres_port" \
    -c unix_socket_directories= -c max_connections=32 -c shared_buffers=16MB \
    >>"$evidence_dir/postgres.log" 2>&1 &
  postgres_pid=$!
  for _ in {1..100}; do
    kill -0 "$postgres_pid" 2>/dev/null || { printf 'Owned PostgreSQL process exited.\n' >&2; return 1; }
    if env -i PATH="$PATH" PGCONNECT_TIMEOUT=1 "$psql_bin" "$database_url" -X -tAc 'SELECT 1' >"$evidence_dir/postgres-probe.log" 2>&1; then
      return 0
    fi
    sleep 0.1
  done
  printf 'Owned PostgreSQL did not start.\n' >&2
  return 1
}

stop_postgres() {
  "$pg_ctl_bin" -D "$fixture_dir/postgres" stop -m fast -w -t 10 >>"$evidence_dir/postgres-stop.log" 2>&1
  wait "$postgres_pid"
  postgres_pid=""
}

pause_postgres() {
  # Freeze the owning postmaster before its children, preventing newly spawned
  # backends from bypassing the timeout fixture. All PIDs belong to our cluster.
  kill -STOP "$postgres_pid"
  postgres_suspended_pids=("$postgres_pid")
  while IFS= read -r child_pid; do
    [[ -n "$child_pid" ]] || continue
    if kill -STOP "$child_pid" 2>/dev/null; then
      postgres_suspended_pids+=("$child_pid")
    fi
  done < <(pgrep -P "$postgres_pid")
}

resume_postgres() {
  kill -CONT "${postgres_suspended_pids[@]}"
  postgres_suspended_pids=()
}

start_redis() {
  "$redis_bin" --bind 127.0.0.1 --port "$redis_port" --protected-mode yes \
    --save '' --appendonly no --dir "$fixture_dir" >>"$evidence_dir/redis.log" 2>&1 &
  redis_pid=$!
  # An actual RESP PING avoids a TCP-open check accepting an unrelated service.
  for _ in {1..100}; do
    kill -0 "$redis_pid" 2>/dev/null || { printf 'Owned Redis process exited.\n' >&2; return 1; }
    if node -e '
const net = require("node:net");
const socket = net.connect({host: "127.0.0.1", port: Number(process.argv[1])});
let response = "";
socket.setTimeout(500, () => process.exit(1));
socket.on("error", () => process.exit(1));
socket.on("connect", () => socket.write("*1\r\n$4\r\nPING\r\n"));
socket.on("data", data => { response += data; if (response.includes("\r\n")) process.exit(response === "+PONG\r\n" ? 0 : 1); });
' "$redis_port"; then
      return 0
    fi
    sleep 0.1
  done
  printf 'Owned Redis did not start.\n' >&2
  return 1
}

observe() {
  local phase="$1" attempt="$2" endpoint="$3"
  local record="$evidence_dir/$phase-$attempt-$endpoint"
  local measured
  measured="$(curl --silent --show-error --noproxy '*' --connect-timeout 1 --max-time 2 \
    --dump-header "$record.headers" --output "$record.json" \
    --write-out '%{http_code} %{time_total}' "$base_url/$endpoint" 2>"$record.stderr")" || true
  read -r observed_status observed_seconds <<<"$measured"
  observed_status="${observed_status:-000}"
  printf '%s\t%s\t%s\t%s\t%s\n' "$phase" "$attempt" "$endpoint" "$observed_status" "${observed_seconds:-0}" >>"$evidence_dir/http-observations.tsv"
  observed_body="$record.json"
}

assert_safe_readiness() {
  # Restrict dependency objects to stable public statuses, with no raw errors,
  # URLs, credentials, or database/Redis exception details in the response.
  jq -e '
    keys == ["component", "dependencies", "gate_readiness", "lifecycle_status", "manifest_path", "manifest_version", "status", "warmup_status", "workers"] and
    .component == "aether-gateway" and
    (.status == "ready" or .status == "not_ready") and
    (.gate_readiness | type == "boolean") and
    (.lifecycle_status == "starting" or .lifecycle_status == "running" or .lifecycle_status == "closing") and
    (.dependencies | keys == ["database", "redis"]) and
    all(.dependencies[];
      keys == ["required", "status"] and
      (.required | type == "boolean") and
      (.status == "ok" or .status == "failed" or .status == "timeout" or .status == "disabled" or .status == "unchecked")) and
    (.workers | keys == ["usage_counter_flush", "usage_queue"]) and
    all(.workers[];
      keys == ["required", "status"] and
      (.required | type == "boolean") and
      (.status == "ok" or .status == "failed" or .status == "disabled")) and
    ([.. | strings] | all(test("postgres://|postgresql://|redis://|127\\.0\\.0\\.1|password|SELECT 1|connection refused"; "i") | not))
  ' "$observed_body" >/dev/null
}

assert_health() {
  local phase="$1" attempt="$2"
  observe "$phase" "$attempt" health
  [[ "$observed_status" == 200 ]] && jq -e '.status == "healthy"' "$observed_body" >/dev/null
}

await_readiness() {
  local phase="$1" expected_http="$2" database_status="$3" redis_status="$4" allowance="$5"
  local deadline=$((SECONDS + allowance)) attempt=0
  while (( SECONDS < deadline )); do
    attempt=$((attempt + 1))
    kill -0 "$gateway_pid" 2>/dev/null || { printf 'Gateway exited during %s.\n' "$phase" >&2; return 1; }
    observe "$phase" "$attempt" readyz
    if [[ "$observed_status" != 000 ]]; then
      assert_safe_readiness || { printf 'Unsafe or invalid readiness body during %s.\n' "$phase" >&2; return 1; }
      # Permit scheduler/HTTP overhead while rejecting the former two-second
      # sequential dependency deadline. Exact boundary timing is tested with virtual time.
      jq -en --argjson elapsed "$observed_seconds" '$elapsed < 1.5' >/dev/null || {
        printf 'Readiness exceeded the one-second deadline plus scheduling allowance during %s.\n' "$phase" >&2
        return 1
      }
    fi
    if [[ "$observed_status" == "$expected_http" ]] && jq -e \
      --arg database "$database_status" --arg redis "$redis_status" --arg http "$expected_http" '
      .gate_readiness == ($http == "200") and
      .lifecycle_status == "running" and
      .status == (if $http == "200" then "ready" else "not_ready" end) and
      .dependencies.database.required == true and .dependencies.redis.required == true and
      (.dependencies.database.status | test($database)) and
      (.dependencies.redis.status | test($redis))
      ' "$observed_body" >/dev/null; then
      assert_health "$phase" "$attempt" || { printf 'Liveness failed during %s.\n' "$phase" >&2; return 1; }
      printf 'PASS %s: readiness=%s, liveness=200\n' "$phase" "$expected_http" | tee -a "$evidence_dir/result.log"
      return 0
    fi
    if [[ "$phase" != initial ]]; then
      assert_health "$phase" "$attempt" || { printf 'Liveness failed while waiting for %s.\n' "$phase" >&2; return 1; }
    fi
    sleep 0.1
  done
  printf 'Readiness deadline exceeded during %s; inspect %s.\n' "$phase" "$evidence_dir" >&2
  return 1
}

start_postgres
start_redis
(
  cd "$fixture_dir"
  exec env -i PATH="$PATH" \
    JWT_SECRET_KEY=aether-readiness-disposable-jwt-key-0001 \
    ENCRYPTION_KEY=aether-readiness-disposable-data-key-0001 \
    ADMIN_USERNAME=readiness-admin ADMIN_EMAIL=readiness@example.invalid \
    ADMIN_PASSWORD=ReadinessDisposablePassword123! \
    AETHER_DATABASE_DRIVER=postgres AETHER_DATABASE_URL="$database_url" \
    AETHER_GATEWAY_DATA_REDIS_URL="$redis_url" AETHER_RUNTIME_BACKEND=redis \
    AETHER_GATEWAY_DATA_POSTGRES_MIN_CONNECTIONS=1 AETHER_GATEWAY_DATA_POSTGRES_MAX_CONNECTIONS=8 \
    AETHER_GATEWAY_HTTP_SHUTDOWN_TIMEOUT_MS=1000 AETHER_GATEWAY_USAGE_SHUTDOWN_TIMEOUT_MS=1000 \
    AETHER_GATEWAY_READINESS_WITHDRAWAL_DELAY_MS=3000 \
    "$gateway_bin" --app-port "$gateway_port" --listener-shards 1 --max-in-flight-requests 8 --database-mode auto
) >"$evidence_dir/gateway.log" 2>&1 &
gateway_pid=$!

await_readiness initial 200 '^ok$' '^ok$' 180
stop_postgres
await_readiness database_stopped 503 '^(failed|timeout)$' '^ok$' 20
start_postgres
await_readiness database_recovered 200 '^ok$' '^ok$' 30
pause_postgres
await_readiness database_unresponsive 503 '^timeout$' '^ok$' 20
resume_postgres
await_readiness database_resumed 200 '^ok$' '^ok$' 30
stop_child "$redis_pid"
redis_pid=""
await_readiness redis_stopped 503 '^ok$' '^(failed|timeout)$' 20
start_redis
await_readiness redis_recovered 200 '^ok$' '^ok$' 30
kill -STOP "$redis_pid"
await_readiness redis_unresponsive 503 '^ok$' '^timeout$' 20
kill -CONT "$redis_pid"
await_readiness redis_resumed 200 '^ok$' '^ok$' 30
pause_postgres
kill -STOP "$redis_pid"
await_readiness both_unresponsive 503 '^timeout$' '^timeout$' 20
resume_postgres
kill -CONT "$redis_pid"
await_readiness both_resumed 200 '^ok$' '^ok$' 30

# A cached successful result must not override the closing gate. Observe the
# real binary's signal path while its listener is still serving the withdrawal window.
kill -TERM "$gateway_pid"
closing_observed=false
for attempt in {1..15}; do
  observe closing "$attempt" readyz
  if [[ "$observed_status" == 503 ]] && jq -e '.lifecycle_status == "closing" and .gate_readiness == false' "$observed_body" >/dev/null; then
    assert_safe_readiness
    assert_health closing "$attempt"
    closing_observed=true
    break
  fi
  sleep 0.1
done
[[ "$closing_observed" == true ]] || { printf 'Closing readiness was not observable before listener drain.\n' >&2; exit 1; }
printf 'PASS closing: readiness=503, liveness=200 before listener drain\n' | tee -a "$evidence_dir/result.log"
for _ in {1..100}; do
  kill -0 "$gateway_pid" 2>/dev/null || break
  sleep 0.1
done
if kill -0 "$gateway_pid" 2>/dev/null; then
  printf 'Gateway did not exit within its shutdown budget.\n' >&2
  exit 1
fi
wait "$gateway_pid"
gateway_pid=""
printf 'PASS: real gateway readiness withdrawal, recovery, both dependency deadlines, closing gate, and independent liveness\n' | tee -a "$evidence_dir/result.log"
