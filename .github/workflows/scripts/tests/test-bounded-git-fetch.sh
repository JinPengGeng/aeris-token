#!/usr/bin/env bash
set -euo pipefail

SCRIPT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_BASE="${AERIS_TEST_TMP_ROOT:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}"
mkdir -p "${RUN_BASE}"
RUN_ROOT="$(mktemp -d "${RUN_BASE%/}/aeris-bounded-fetch-test.XXXXXX")"
SOURCE="${RUN_ROOT}/source"
REMOTE="${RUN_ROOT}/remote.git"
STAGES="${RUN_ROOT}/stages"
mkdir -p "${SOURCE}" "${STAGES}"

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

expect_rejected() {
  local label="$1"
  shift
  if "$@" >/dev/null 2>&1; then
    fail "${label} was accepted"
  fi
}

assert_stages_clean() {
  local residue
  residue="$(find "${STAGES}" -mindepth 1 -maxdepth 1 -type d -name 'aeris-bounded-fetch.*' -print -quit)"
  [[ -z "${residue}" ]] || fail "isolated receiver was not cleaned: ${residue}"
  residue="$(find "${STAGES}" -mindepth 1 -maxdepth 1 -type f -name 'aeris-remote-ref.*' -print -quit)"
  [[ -z "${residue}" ]] || fail "bounded remote-ref output was not cleaned: ${residue}"
}

git init -q --bare "${REMOTE}"
git init -q "${SOURCE}"
cd "${SOURCE}"
git config user.name 'Bounded Fetch Fixture'
git config user.email 'bounded-fetch@example.com'
git config core.autocrlf false
printf 'base' >a.txt
printf 'base' >b.txt
git add .
git commit -qm base
BASE="$(git rev-parse HEAD)"

printf '1234' >a.txt
printf '5678' >b.txt
git commit -qam 'exact delta boundary'
TIP="$(git rev-parse HEAD)"
git branch exact "${TIP}"

git switch -q -c blob-over "${BASE}"
printf '12345' >a.txt
git commit -qam 'blob over boundary'
BLOB_OVER="$(git rev-parse HEAD)"

git switch -q -c total-over "${BASE}"
printf '12345' >a.txt
printf '5678' >b.txt
git commit -qam 'total over boundary'
TOTAL_OVER="$(git rev-parse HEAD)"

git switch -q -c paths-over "${BASE}"
printf '1' >a.txt
printf '2' >b.txt
printf '3' >c.txt
git add .
git commit -qm 'paths over boundary'
PATHS_OVER="$(git rev-parse HEAD)"

git switch -q -c compressed-over "${BASE}"
head -c 1048576 /dev/zero >compressed.bin
git add compressed.bin
git commit -qm 'high compression expansion'
COMPRESSED_OVER="$(git rev-parse HEAD)"

git switch -q -c delta-over "${BASE}"
head -c 262144 /dev/urandom >delta.bin
git add delta.bin
git commit -qm 'delta base object'
DELTA_BASE_BLOB="$(git rev-parse HEAD:delta.bin)"
printf X | dd of=delta.bin bs=1 seek=131072 conv=notrunc status=none
git commit -qam 'delta expansion tip'
DELTA_OVER="$(git rev-parse HEAD)"
DELTA_BLOB="$(git rev-parse HEAD:delta.bin)"

git push -q "${REMOTE}" \
  "${TIP}:refs/heads/exact" \
  "${BLOB_OVER}:refs/heads/blob-over" \
  "${TOTAL_OVER}:refs/heads/total-over" \
  "${PATHS_OVER}:refs/heads/paths-over" \
  "${COMPRESSED_OVER}:refs/heads/compressed-over" \
  "${DELTA_OVER}:refs/heads/delta-over"
git -C "${REMOTE}" repack -adq
REMOTE_PACK_INDEX="$(find "${REMOTE}/objects/pack" -type f -name '*.idx' -print -quit)"
git verify-pack -v "${REMOTE_PACK_INDEX}" |
  awk -v first="${DELTA_BASE_BLOB}" -v second="${DELTA_BLOB}" \
    '($1 == first || $1 == second) && NF >= 7 { found = 1 } END { exit !found }' ||
  fail 'delta expansion fixture was not stored as a pack delta'

new_receiver() {
  local name="$1"
  local path="${RUN_ROOT}/${name}"
  git init -q "${path}"
  printf '%s\n' "${path}"
}

reset_totals() {
  AERIS_FETCH_RECEIVED_BYTES_TOTAL=0
  AERIS_FETCH_RECEIVED_EXPANDED_BYTES_TOTAL=0
  AERIS_FETCH_RECEIVED_OBJECTS_TOTAL=0
  AERIS_FETCH_IMPORT_BYTES_TOTAL=0
  AERIS_FETCH_IMPORT_OBJECTS_TOTAL=0
  AERIS_BOUNDED_LAST_RECEIVED_BYTES=0
  AERIS_BOUNDED_LAST_RECEIVED_EXPANDED_BYTES=0
  AERIS_BOUNDED_LAST_RECEIVED_OBJECTS=0
  AERIS_BOUNDED_LAST_IMPORT_BYTES=0
  AERIS_BOUNDED_LAST_IMPORT_OBJECTS=0
}

export AERIS_BOUNDED_FETCH_TEST_MODE=true
export AERIS_BOUNDED_FETCH_TEST_FIXTURE=true
export AERIS_BOUNDED_FETCH_TMP_ROOT="${STAGES}"
source "${SCRIPT_ROOT}/bounded-git-fetch.sh"

RECEIVER_ONE="$(new_receiver receiver-one)"
cd "${RECEIVER_ONE}"
aeris_bounded_fetch_init /dev/null

OVERSIZED_BIN="${RUN_ROOT}/oversized-bin"
REAL_GIT="$(command -v git)"
mkdir -p "${OVERSIZED_BIN}"
cat >"${OVERSIZED_BIN}/git" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
if [[ " $* " == *' ls-remote '* ]]; then
  head -c 8192 /dev/zero | tr '\0' x
  exit 0
fi
exec "${REAL_GIT}" "$@"
EOF
chmod +x "${OVERSIZED_BIN}/git"
expect_rejected 'oversized remote advertisement' env \
  PATH="${OVERSIZED_BIN}:${PATH}" REAL_GIT="${REAL_GIT}" \
  bash -c 'source "$1"; export AERIS_BOUNDED_FETCH_TEST_MODE=true AERIS_BOUNDED_FETCH_TEST_FIXTURE=true AERIS_BOUNDED_FETCH_TMP_ROOT="$2"; aeris_bounded_read_remote_ref ignored refs/heads/exact oversized' \
  bash "${SCRIPT_ROOT}/bounded-git-fetch.sh" "${STAGES}"
assert_stages_clean

aeris_bounded_read_remote_ref "${REMOTE}" refs/heads/exact exact
[[ "${AERIS_BOUNDED_REMOTE_SHA}" == "${TIP}" ]] || fail 'exact ref discovery returned the wrong SHA'
aeris_bounded_fetch_ref "${REMOTE}" refs/heads/exact "${TIP}" refs/aeris/test/exact exact
[[ "$(git rev-parse refs/aeris/test/exact)" == "${TIP}" ]] || fail 'positive exact fetch missed its tip'
[[ ! -e "$(git rev-parse --git-path FETCH_HEAD)" ]] ||
  fail 'quarantine import mutated FETCH_HEAD before CAS publication'
RECEIVED_BYTES="${AERIS_BOUNDED_LAST_RECEIVED_BYTES}"
RECEIVED_EXPANDED_BYTES="${AERIS_BOUNDED_LAST_RECEIVED_EXPANDED_BYTES}"
RECEIVED_OBJECTS="${AERIS_BOUNDED_LAST_RECEIVED_OBJECTS}"
IMPORTED_BYTES="${AERIS_BOUNDED_LAST_IMPORT_BYTES}"
IMPORTED_OBJECTS="${AERIS_BOUNDED_LAST_IMPORT_OBJECTS}"
((RECEIVED_BYTES > 0 && RECEIVED_EXPANDED_BYTES > 0 && RECEIVED_OBJECTS > 0 &&
   IMPORTED_BYTES > 0 && IMPORTED_OBJECTS > 0)) || fail 'positive fetch had incomplete resource measurements'
assert_stages_clean

RECEIVER_EXPANDED="$(new_receiver receiver-expanded)"
cd "${RECEIVER_EXPANDED}"
reset_totals
AERIS_FETCH_MAX_RECEIVED_EXPANDED_BYTES="${RECEIVED_EXPANDED_BYTES}"
aeris_bounded_fetch_ref "${REMOTE}" refs/heads/exact "${TIP}" refs/aeris/test/exact exact
assert_stages_clean

RECEIVER_EXPANDED_OVER="$(new_receiver receiver-expanded-over)"
cd "${RECEIVER_EXPANDED_OVER}"
reset_totals
AERIS_FETCH_MAX_RECEIVED_EXPANDED_BYTES=$((RECEIVED_EXPANDED_BYTES - 1))
expect_rejected 'expanded-byte over-boundary fetch' \
  aeris_bounded_fetch_ref "${REMOTE}" refs/heads/exact "${TIP}" refs/aeris/test/exact exact
git show-ref --verify --quiet refs/aeris/test/exact && fail 'expanded-byte rejection imported a destination ref'
assert_stages_clean
AERIS_FETCH_MAX_RECEIVED_EXPANDED_BYTES=1073741824

RECEIVER_DELTA="$(new_receiver receiver-delta-over)"
cd "${RECEIVER_DELTA}"
reset_totals
AERIS_FETCH_MAX_OBJECT_BYTES=1048576
AERIS_FETCH_MAX_BLOB_BYTES=1048576
AERIS_FETCH_MAX_RECEIVED_EXPANDED_BYTES=400000
expect_rejected 'cumulative delta expansion' \
  aeris_bounded_fetch_ref "${REMOTE}" refs/heads/delta-over "${DELTA_OVER}" \
    refs/aeris/test/delta-over delta-over
git show-ref --verify --quiet refs/aeris/test/delta-over && fail 'delta rejection imported a destination ref'
assert_stages_clean
AERIS_FETCH_MAX_OBJECT_BYTES=33554432
AERIS_FETCH_MAX_BLOB_BYTES=33554432
AERIS_FETCH_MAX_RECEIVED_EXPANDED_BYTES=1073741824

RECEIVER_IMPORT="$(new_receiver receiver-import)"
cd "${RECEIVER_IMPORT}"
reset_totals
AERIS_FETCH_MAX_IMPORT_BYTES="${IMPORTED_BYTES}"
AERIS_FETCH_MAX_IMPORT_OBJECTS="${IMPORTED_OBJECTS}"
aeris_bounded_fetch_ref "${REMOTE}" refs/heads/exact "${TIP}" refs/aeris/test/exact exact
assert_stages_clean

RECEIVER_IMPORT_OVER="$(new_receiver receiver-import-over)"
cd "${RECEIVER_IMPORT_OVER}"
git config user.name 'Bounded Fetch Fixture'
git config user.email 'bounded-fetch@example.com'
printf 'preserve' >preserve.txt
git add preserve.txt
git commit -qm 'preserved destination'
PRESERVED_DESTINATION="$(git rev-parse HEAD)"
git update-ref refs/aeris/test/exact "${PRESERVED_DESTINATION}"
reset_totals
AERIS_FETCH_MAX_IMPORT_BYTES=$((IMPORTED_BYTES - 1))
AERIS_FETCH_MAX_IMPORT_OBJECTS=250000
expect_rejected 'caller import expansion over boundary' \
  aeris_bounded_fetch_ref "${REMOTE}" refs/heads/exact "${TIP}" refs/aeris/test/exact exact
[[ "$(git rev-parse refs/aeris/test/exact)" == "${PRESERVED_DESTINATION}" ]] ||
  fail 'import rejection did not restore the previous destination ref'
git cat-file -e "${PRESERVED_DESTINATION}^{commit}" ||
  fail 'import rejection removed an object reachable from the previous destination ref'
git cat-file -e "${TIP}^{commit}" 2>/dev/null &&
  fail 'import rejection retained newly imported objects'
assert_stages_clean
AERIS_FETCH_MAX_IMPORT_BYTES=268435456

RECEIVER_CONCURRENT="$(new_receiver receiver-concurrent)"
cd "${RECEIVER_CONCURRENT}"
git config user.name 'Bounded Fetch Fixture'
git config user.email 'bounded-fetch@example.com'
printf 'previous' >race.txt
git add race.txt
git commit -qm 'previous destination'
RACE_PREVIOUS="$(git rev-parse HEAD)"
printf 'concurrent' >race.txt
git commit -qam 'concurrent destination'
RACE_CONCURRENT="$(git rev-parse HEAD)"
git update-ref refs/aeris/test/exact "${RACE_PREVIOUS}"
reset_totals
CONCURRENT_OBJECT=''
inject_concurrent_publish() {
  local destination="$1" expected="$2" previous_ref="$3"
  [[ "${previous_ref}" == "${RACE_PREVIOUS}" && "${expected}" == "${TIP}" ]] || return 1
  CONCURRENT_OBJECT="$(printf 'concurrent-object' | git hash-object -w --stdin)"
  git update-ref "${destination}" "${RACE_CONCURRENT}" "${previous_ref}"
}
AERIS_BOUNDED_IMPORT_PRE_PUBLISH_HOOK=inject_concurrent_publish
expect_rejected 'concurrent destination CAS conflict' \
  aeris_bounded_fetch_ref "${REMOTE}" refs/heads/exact "${TIP}" refs/aeris/test/exact exact
unset AERIS_BOUNDED_IMPORT_PRE_PUBLISH_HOOK
[[ "$(git rev-parse refs/aeris/test/exact)" == "${RACE_CONCURRENT}" ]] ||
  fail 'CAS conflict overwrote the concurrent destination ref'
git cat-file -e "${CONCURRENT_OBJECT}^{blob}" ||
  fail 'failed import deleted a concurrently written object'
git cat-file -e "${TIP}^{commit}" ||
  fail 'CAS conflict rolled back immutable imported objects'
assert_stages_clean

RECEIVER_FINAL_SHA="$(new_receiver receiver-final-sha)"
cd "${RECEIVER_FINAL_SHA}"
git config user.name 'Bounded Fetch Fixture'
git config user.email 'bounded-fetch@example.com'
printf 'previous' >race.txt
git add race.txt
git commit -qm 'previous destination'
FINAL_PREVIOUS="$(git rev-parse HEAD)"
printf 'concurrent' >race.txt
git commit -qam 'post-publication concurrent destination'
FINAL_CONCURRENT="$(git rev-parse HEAD)"
git update-ref refs/aeris/test/exact "${FINAL_PREVIOUS}"
reset_totals
replace_after_publish() {
  local destination="$1" expected="$2" previous_ref="$3"
  [[ "${previous_ref}" == "${FINAL_PREVIOUS}" ]] || return 1
  git update-ref "${destination}" "${FINAL_CONCURRENT}" "${expected}"
}
AERIS_BOUNDED_IMPORT_POST_PUBLISH_HOOK=replace_after_publish
expect_rejected 'final exact-SHA publication drift' \
  aeris_bounded_fetch_ref "${REMOTE}" refs/heads/exact "${TIP}" refs/aeris/test/exact exact
unset AERIS_BOUNDED_IMPORT_POST_PUBLISH_HOOK
[[ "$(git rev-parse refs/aeris/test/exact)" == "${FINAL_CONCURRENT}" ]] ||
  fail 'final exact-SHA failure corrupted the concurrent destination ref'
git cat-file -e "${TIP}^{commit}" ||
  fail 'final exact-SHA failure removed already imported immutable objects'
assert_stages_clean

RECEIVER_BYTES="$(new_receiver receiver-bytes)"
cd "${RECEIVER_BYTES}"
reset_totals
AERIS_FETCH_MAX_RECEIVED_BYTES="${RECEIVED_BYTES}"
AERIS_FETCH_MAX_RECEIVED_OBJECTS=250000
aeris_bounded_fetch_init /dev/null
aeris_bounded_fetch_ref "${REMOTE}" refs/heads/exact "${TIP}" refs/aeris/test/exact exact
assert_stages_clean

RECEIVER_BYTES_OVER="$(new_receiver receiver-bytes-over)"
cd "${RECEIVER_BYTES_OVER}"
reset_totals
AERIS_FETCH_MAX_RECEIVED_BYTES=$((RECEIVED_BYTES - 1))
expect_rejected 'received-byte over-boundary fetch' \
  aeris_bounded_fetch_ref "${REMOTE}" refs/heads/exact "${TIP}" refs/aeris/test/exact exact
git show-ref --verify --quiet refs/aeris/test/exact && fail 'byte rejection imported a destination ref'
assert_stages_clean

add_stage_padding() {
  head -c 1024 /dev/zero >"$1/non-object-padding"
}
RECEIVER_STAGE_BYTES_OVER="$(new_receiver receiver-stage-bytes-over)"
cd "${RECEIVER_STAGE_BYTES_OVER}"
reset_totals
AERIS_FETCH_MAX_RECEIVED_BYTES=$((RECEIVED_BYTES + 100))
AERIS_BOUNDED_STAGE_POST_HOOK=add_stage_padding
expect_rejected 'whole-stage byte over-boundary fetch' \
  aeris_bounded_fetch_ref "${REMOTE}" refs/heads/exact "${TIP}" refs/aeris/test/exact exact
unset AERIS_BOUNDED_STAGE_POST_HOOK
git show-ref --verify --quiet refs/aeris/test/exact && fail 'whole-stage rejection imported a destination ref'
assert_stages_clean

RECEIVER_OBJECTS="$(new_receiver receiver-objects)"
cd "${RECEIVER_OBJECTS}"
reset_totals
AERIS_FETCH_MAX_RECEIVED_BYTES=268435456
AERIS_FETCH_MAX_RECEIVED_OBJECTS="${RECEIVED_OBJECTS}"
aeris_bounded_fetch_ref "${REMOTE}" refs/heads/exact "${TIP}" refs/aeris/test/exact exact
assert_stages_clean

RECEIVER_OBJECTS_OVER="$(new_receiver receiver-objects-over)"
cd "${RECEIVER_OBJECTS_OVER}"
reset_totals
AERIS_FETCH_MAX_RECEIVED_OBJECTS=$((RECEIVED_OBJECTS - 1))
expect_rejected 'received-object over-boundary fetch' \
  aeris_bounded_fetch_ref "${REMOTE}" refs/heads/exact "${TIP}" refs/aeris/test/exact exact
assert_stages_clean

RECEIVER_BLOB="$(new_receiver receiver-blob-over)"
cd "${RECEIVER_BLOB}"
reset_totals
AERIS_FETCH_MAX_RECEIVED_OBJECTS=250000
AERIS_FETCH_MAX_BLOB_BYTES=3
expect_rejected 'received blob over-boundary fetch' \
  aeris_bounded_fetch_ref "${REMOTE}" refs/heads/exact "${TIP}" refs/aeris/test/exact exact
assert_stages_clean

RECEIVER_MISMATCH="$(new_receiver receiver-mismatch)"
cd "${RECEIVER_MISMATCH}"
reset_totals
AERIS_FETCH_MAX_BLOB_BYTES=33554432
expect_rejected 'exact ref SHA mismatch' \
  aeris_bounded_fetch_ref "${REMOTE}" refs/heads/exact "${BASE}" refs/aeris/test/exact exact
assert_stages_clean

RECEIVER_COMPRESSED="$(new_receiver receiver-compressed-over)"
cd "${RECEIVER_COMPRESSED}"
reset_totals
AERIS_FETCH_MAX_OBJECT_BYTES=2097152
AERIS_FETCH_MAX_BLOB_BYTES=2097152
AERIS_FETCH_MAX_RECEIVED_EXPANDED_BYTES=100000
expect_rejected 'high-compression cumulative expansion' \
  aeris_bounded_fetch_ref "${REMOTE}" refs/heads/compressed-over "${COMPRESSED_OVER}" \
    refs/aeris/test/compressed-over compressed-over
git show-ref --verify --quiet refs/aeris/test/compressed-over && fail 'compression rejection imported a destination ref'
assert_stages_clean
AERIS_FETCH_MAX_OBJECT_BYTES=33554432
AERIS_FETCH_MAX_BLOB_BYTES=33554432
AERIS_FETCH_MAX_RECEIVED_EXPANDED_BYTES=1073741824

RECEIVER_STALL="$(new_receiver receiver-stall)"
cd "${RECEIVER_STALL}"
reset_totals
AERIS_FETCH_TIMEOUT_SECONDS=1
AERIS_TEST_FETCH_DELAY_SECONDS=2
expect_rejected 'stalled fetch' \
  aeris_bounded_fetch_ref "${REMOTE}" refs/heads/exact "${TIP}" refs/aeris/test/exact exact
unset AERIS_TEST_FETCH_DELAY_SECONDS
AERIS_FETCH_TIMEOUT_SECONDS=90
assert_stages_clean

RECEIVER_UNENFORCED="$(new_receiver receiver-unenforced)"
cd "${RECEIVER_UNENFORCED}"
reset_totals
GITHUB_ACTIONS=true
AERIS_BOUNDED_TEST_DISABLE_LIMITS=true
expect_rejected 'Linux runner without enforceable resource limits' \
  aeris_bounded_fetch_ref "${REMOTE}" refs/heads/exact "${TIP}" refs/aeris/test/exact exact
unset GITHUB_ACTIONS AERIS_BOUNDED_TEST_DISABLE_LIMITS
git show-ref --verify --quiet refs/aeris/test/exact &&
  fail 'unenforced runner rejection published a destination ref'
assert_stages_clean

CORRUPT="${RUN_ROOT}/corrupt.git"
git clone -q --bare --no-hardlinks "${REMOTE}" "${CORRUPT}"
PACK="$(find "${CORRUPT}/objects/pack" -type f -name '*.pack' -print -quit)"
[[ -n "${PACK}" ]] || {
  git -C "${CORRUPT}" repack -adq
  PACK="$(find "${CORRUPT}/objects/pack" -type f -name '*.pack' -print -quit)"
}
chmod u+w "${PACK}"
printf 'corrupt' >"${PACK}"
RECEIVER_CORRUPT="$(new_receiver receiver-corrupt)"
cd "${RECEIVER_CORRUPT}"
reset_totals
expect_rejected 'corrupt remote object graph' \
  aeris_bounded_fetch_ref "${CORRUPT}" refs/heads/exact "${TIP}" refs/aeris/test/exact exact
assert_stages_clean

SHALLOW="${RUN_ROOT}/shallow"
git clone -q --depth=1 --branch exact "file://${REMOTE}" "${SHALLOW}"
cd "${SHALLOW}"
expect_rejected 'shallow checkpoint repository' aeris_bounded_fetch_init /dev/null
LINKED_SHALLOW="${RUN_ROOT}/linked-shallow"
git worktree add -q --detach "${LINKED_SHALLOW}" HEAD
SHALLOW_HEAD="$(git rev-parse HEAD)"
SHALLOW_OBJECT="$(git rev-parse HEAD^{tree})"
cd "${LINKED_SHALLOW}"
reset_totals
AERIS_BOUNDED_BOOTSTRAP_SHALLOW=true
expect_rejected 'linked shallow bootstrap with shared common directory' \
  aeris_bounded_fetch_init /dev/null
unset AERIS_BOUNDED_BOOTSTRAP_SHALLOW
[[ "$(git rev-parse HEAD)" == "${SHALLOW_HEAD}" ]] ||
  fail 'linked bootstrap rejection deleted a shared ref'
git cat-file -e "${SHALLOW_OBJECT}^{tree}" ||
  fail 'linked bootstrap rejection deleted a shared object'
[[ "$(git rev-parse --is-shallow-repository)" == true ]] ||
  fail 'linked bootstrap rejection removed the shared shallow boundary'
cd "${SHALLOW}"
AERIS_BOUNDED_BOOTSTRAP_SHALLOW=true
expect_rejected 'primary shallow bootstrap while a linked worktree exists' \
  aeris_bounded_fetch_init /dev/null
unset AERIS_BOUNDED_BOOTSTRAP_SHALLOW
git worktree remove --force "${LINKED_SHALLOW}"
reset_totals
AERIS_BOUNDED_BOOTSTRAP_SHALLOW=true
aeris_bounded_fetch_init /dev/null
[[ "$(git rev-parse --is-shallow-repository)" == false ]] ||
  fail 'explicit bootstrap conversion retained the shallow boundary'
aeris_bounded_fetch_ref "${REMOTE}" refs/heads/exact "${TIP}" refs/aeris/test/bootstrap exact
git fsck --strict --no-reflogs --no-dangling "${TIP}" >/dev/null ||
  fail 'bounded bootstrap repair did not restore the complete exact-ref graph'
unset AERIS_BOUNDED_BOOTSTRAP_SHALLOW

cd "${SOURCE}"
AERIS_FETCH_MAX_CHANGED_PATHS=2
AERIS_FETCH_MAX_BLOB_BYTES=4
AERIS_FETCH_MAX_CHANGED_BLOB_BYTES=8
aeris_enforce_change_bounds "${BASE}" "${TIP}" 'exact delta boundary'
expect_rejected 'changed blob over boundary' \
  aeris_enforce_change_bounds "${BASE}" "${BLOB_OVER}" 'blob delta'
AERIS_FETCH_MAX_BLOB_BYTES=5
expect_rejected 'total changed-blob bytes over boundary' \
  aeris_enforce_change_bounds "${BASE}" "${TOTAL_OVER}" 'total delta'
AERIS_FETCH_MAX_CHANGED_BLOB_BYTES=268435456
expect_rejected 'changed path count over boundary' \
  aeris_enforce_change_bounds "${BASE}" "${PATHS_OVER}" 'path delta'
expect_rejected 'unavailable checkpoint ancestry' \
  aeris_enforce_change_bounds 0000000000000000000000000000000000000000 "${TIP}" 'missing checkpoint'

AERIS_FETCH_MAX_CHANGED_PATHS=20000
AERIS_FETCH_MAX_TREE_ENTRIES=2
expect_rejected 'tree entry traversal over boundary' \
  aeris_enforce_change_bounds "${BASE}" "${PATHS_OVER}" 'tree entry delta'
AERIS_FETCH_MAX_TREE_ENTRIES=500000
AERIS_FETCH_MAX_DIFF_BYTES=16
expect_rejected 'changed-path output over boundary' \
  aeris_enforce_change_bounds "${BASE}" "${TIP}" 'diff output delta'
AERIS_FETCH_MAX_DIFF_BYTES=33554432

ALTERNATE_RECEIVER="$(new_receiver receiver-alternates)"
mkdir -p "${ALTERNATE_RECEIVER}/.git/objects/info"
printf '%s\n' "${SOURCE}/.git/objects" >"${ALTERNATE_RECEIVER}/.git/objects/info/alternates"
cd "${ALTERNATE_RECEIVER}"
expect_rejected 'alternate object store' aeris_bounded_fetch_init /dev/null

PROMISOR_RECEIVER="$(new_receiver receiver-promisor)"
git -C "${PROMISOR_RECEIVER}" config remote.origin.promisor true
cd "${PROMISOR_RECEIVER}"
expect_rejected 'promisor repository' aeris_bounded_fetch_init /dev/null

ENV_RECEIVER="$(new_receiver receiver-object-env)"
cd "${ENV_RECEIVER}"
expect_rejected 'object directory environment override' \
  env GIT_OBJECT_DIRECTORY="${RUN_ROOT}/hostile-objects" bash -c \
    'source "$1"; export AERIS_BOUNDED_FETCH_TEST_MODE=true AERIS_BOUNDED_FETCH_TEST_FIXTURE=true; aeris_bounded_fetch_init /dev/null' \
    bash "${SCRIPT_ROOT}/bounded-git-fetch.sh"

unset GIT_SSH GIT_SSH_COMMAND SSH_ASKPASS GIT_ASKPASS GIT_CONFIG_COUNT \
  GIT_CONFIG_PARAMETERS GIT_CONFIG_SYSTEM GIT_CONFIG_GLOBAL GIT_CONFIG \
  GIT_PROXY_COMMAND GIT_HTTP_PROXY_AUTHMETHOD GIT_SSL_NO_VERIFY GIT_SSL_CAINFO \
  GIT_SSL_CAPATH CURL_HOME HTTP_PROXY HTTPS_PROXY ALL_PROXY NO_PROXY \
  http_proxy https_proxy all_proxy no_proxy
for transport_env in GIT_SSH_COMMAND SSH_ASKPASS GIT_CONFIG_COUNT; do
  expect_rejected "credentialless inherited ${transport_env}" \
    env "${transport_env}=hostile" bash -c '
      source "$1"
      export AERIS_BOUNDED_FETCH_TEST_MODE=true AERIS_BOUNDED_FETCH_TEST_FIXTURE=true
      export AERIS_BOUNDED_FETCH_CREDENTIALLESS=true
      aeris_bounded_network_git ls-remote https://github.com/example/repo.git refs/heads/main
    ' bash "${SCRIPT_ROOT}/bounded-git-fetch.sh"
done

for config_case in \
  'core.sshCommand=!hostile' \
  'url.https://attacker.invalid/.insteadOf=https://github.com/' \
  'credential.https://github.com.username=secret-user' \
  'http.proxy=http://127.0.0.1:9' \
  'http.extraHeader=X-Hostile: true' \
  'http.sslVerify=false' \
  'http.sslCAInfo=/tmp/hostile-ca.pem' \
  'http.https://github.com/.sslVerify=false'; do
  key="${config_case%%=*}"
  value="${config_case#*=}"
  git config --local "${key}" "${value}"
  expect_rejected "credentialless Git config ${key}" \
    bash -c '
      source "$1"
      export AERIS_BOUNDED_FETCH_TEST_MODE=true AERIS_BOUNDED_FETCH_TEST_FIXTURE=true
      export AERIS_BOUNDED_FETCH_CREDENTIALLESS=true
      aeris_bounded_network_git ls-remote https://github.com/example/repo.git refs/heads/main
    ' bash "${SCRIPT_ROOT}/bounded-git-fetch.sh"
  git config --local --unset-all "${key}"
done

MULTIPACK_STAGE="${RUN_ROOT}/multi-pack-stage.git"
git clone -q --bare --no-hardlinks "${REMOTE}" "${MULTIPACK_STAGE}"
git -C "${MULTIPACK_STAGE}" repack -adq
MULTIPACK_IDX="$(find "${MULTIPACK_STAGE}/objects/pack" -type f -name '*.idx' -print -quit)"
MULTIPACK_PACK="${MULTIPACK_IDX%.idx}.pack"
MULTIPACK_COUNT="$(git verify-pack -v "${MULTIPACK_IDX}" | awk 'length($1) == 40 && $1 ~ /^[0-9a-f]+$/ { count += 1 } END { print count + 0 }')"
cp "${MULTIPACK_IDX}" "${MULTIPACK_STAGE}/objects/pack/pack-duplicate.idx"
cp "${MULTIPACK_PACK}" "${MULTIPACK_STAGE}/objects/pack/pack-duplicate.pack"
reset_totals
AERIS_FETCH_MAX_RECEIVED_OBJECTS="${MULTIPACK_COUNT}"
expect_rejected 'multi-pack aggregate object count' \
  aeris_bounded_list_stage_objects "${MULTIPACK_STAGE}/objects" "${MULTIPACK_STAGE}/objects.list"
AERIS_FETCH_MAX_RECEIVED_OBJECTS=250000

printf 'PASS bounded Git fetch and delta boundaries (%s)\n' "${RUN_ROOT}"
