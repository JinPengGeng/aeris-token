#!/usr/bin/env bash
set -euo pipefail

SCRIPT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HELPER="${SCRIPT_ROOT}/manage-sync-automerge.sh"
RUN_BASE="${AERIS_TEST_TMP_ROOT:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}}"
mkdir -p "${RUN_BASE}"
RUN_ROOT="$(mktemp -d "${RUN_BASE%/}/aeris-automerge.XXXXXX")"
export AERIS_AUTONOMY_EXPIRES_AT='2099-01-01T00:00:00Z'
export AERIS_WRITER_APP_SLUG='aeris-writer'
export BASE_BRANCH='main'
export SYNC_BRANCH='automation/sync-upstream'
TEST_BASE_SHA='abcdefabcdefabcdefabcdefabcdefabcdefabcd'
TEST_SOURCE='upstream/example@1111111111111111111111111111111111111111'
TEST_CHECKPOINT_SHA='3333333333333333333333333333333333333333'

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

assert_eq() {
  local expected="$1" actual="$2" message="$3"
  [[ "${actual}" == "${expected}" ]] ||
    fail "${message}: expected '${expected}', got '${actual}'"
}

assert_status() {
  local expected="$1" actual="$2" message="$3"
  [[ "${actual}" -eq "${expected}" ]] ||
    fail "${message}: expected status ${expected}, got ${actual}"
}

new_fake_gh() {
  local bin="$1"
  mkdir -p "${bin}"
  printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'printf "%q " "$@" >>"$FAKE_GH_LOG"' \
    'printf "\n" >>"$FAKE_GH_LOG"' \
    'if [[ "${FAKE_GH_STATUS:-0}" != 0 ]]; then exit "$FAKE_GH_STATUS"; fi' \
    'if [[ "$1 $2" == "pr merge" && "${FAKE_GH_MERGE_STATUS:-0}" != 0 ]]; then exit "$FAKE_GH_MERGE_STATUS"; fi' \
    'if [[ "$1" == "api" && "$2" == "--method" && "$3" == "PUT" && "${FAKE_GH_API_STATUS:-0}" != 0 ]]; then exit "$FAKE_GH_API_STATUS"; fi' \
    'if [[ "$1" == "api" && "$2" == "--method" && "$3" == "PUT" ]]; then printf "%s\n" "${FAKE_GH_API_RESPONSE:-}"; fi' \
    'if [[ "$1" == "api" && "$2" == repos/*/pulls/* ]]; then count="$(grep -c "^api repos/.*/pulls/" "$FAKE_GH_LOG")"; if [[ "$count" -eq 1 ]]; then printf "%s\n" "$FAKE_GH_PREFLIGHT_RESPONSE"; else printf "%s\n" "${FAKE_GH_PULL_RESPONSE:-}"; fi; fi' \
    'if [[ "$1 $2" == "api graphql" ]]; then printf "%s\n" "$FAKE_GH_GOVERNANCE_RESPONSE"; fi' \
    'if [[ "$1" == "api" && "$2" == repos/*/rulesets/* ]]; then printf "%s\n" "$FAKE_GH_RULESET_RESPONSE"; fi' \
    'if [[ "$1" == "api" && "$2" == "repos/${FAKE_GH_REPOSITORY}/commits/${FAKE_GH_EXPECTED_HEAD}/check-runs?per_page=100" ]]; then printf "%s\n" "$FAKE_GH_CHECKS_RESPONSE"; fi' \
    'if [[ "$1" == "api" && "$2" == repos/*/commits/* && "$2" != */check-runs\?per_page=100 && "$2" != "repos/${FAKE_GH_REPOSITORY}/commits/${FAKE_GH_EXPECTED_HEAD}" && "${FAKE_GH_COMMIT_STATUS:-0}" != 0 ]]; then exit "$FAKE_GH_COMMIT_STATUS"; fi' \
    'if [[ "$1" == "api" && "$2" == "repos/${FAKE_GH_REPOSITORY}/commits/${FAKE_GH_EXPECTED_HEAD}" ]]; then printf "%s\n" "$FAKE_GH_HEAD_COMMIT_RESPONSE"; fi' \
    'if [[ "$1" == "api" && "$2" == repos/*/commits/* && "$2" != */check-runs\?per_page=100 && "$2" != "repos/${FAKE_GH_REPOSITORY}/commits/${FAKE_GH_EXPECTED_HEAD}" ]]; then printf "%s\n" "${FAKE_GH_COMMIT_RESPONSE:-}"; fi' \
    'if [[ "$1 $2 ${3:-}" == "pr view "* ]]; then if [[ "${FAKE_GH_VIEW_RESPONSE+x}" == x ]]; then printf "%s\n" "${FAKE_GH_VIEW_RESPONSE}"; else printf "%s\n" "${FAKE_GH_AUTO_MERGE:-false}"; fi; fi' \
    >"${bin}/gh"
  chmod +x "${bin}/gh"
}

new_expiring_clock() {
  local bin="$1" clock_calls="$2"
  : >"${clock_calls}"
  cat >"${bin}/date" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "$*" in
  '-u -d 2033-05-18T03:33:20Z +%s') printf '2000000000\n' ;;
  '-u -d @2000000000 +%Y-%m-%dT%H:%M:%SZ') printf '2033-05-18T03:33:20Z\n' ;;
  '-u +%s')
    count="$(wc -l <"${CLOCK_CALLS}")"
    printf 'tick\n' >>"${CLOCK_CALLS}"
    if [[ "${count}" -eq 0 ]]; then printf '1999999999\n'; else printf '2000000000\n'; fi
    ;;
  *) printf 'unexpected date invocation: %s\n' "$*" >&2; exit 1 ;;
esac
EOF
  chmod +x "${bin}/date"
}

default_governance() {
  local number="$1" head="$2" base="$3" repo="$4"
  jq -nc --argjson number "${number}" --arg head "${head}" --arg base "${base}" --arg repo "${repo}" '
    {data:{repository:{
      mergeCommitAllowed:true,
      rebaseMergeAllowed:false,
      squashMergeAllowed:true,
      isArchived:false,
      isDisabled:false,
      isLocked:false,
      branchProtectionRules:{totalCount:0,pageInfo:{hasNextPage:false}},
      rulesets:{
        totalCount:3,
        pageInfo:{hasNextPage:false},
        nodes:[
          {name:"agent-head-fence-v1",enforcement:"ACTIVE",target:"BRANCH",
           conditions:{refName:{include:["refs/heads/agent/**"],exclude:[]}},
           bypassActors:{totalCount:1,pageInfo:{hasNextPage:false},nodes:[{bypassMode:"ALWAYS"}]},
           rules:{totalCount:4,pageInfo:{hasNextPage:false},nodes:[
             {type:"CREATION",parameters:null},
             {type:"UPDATE",parameters:{}},
             {type:"DELETION",parameters:null},
             {type:"NON_FAST_FORWARD",parameters:null}]}},
          {name:"main-linear-history",enforcement:"ACTIVE",target:"BRANCH",
           conditions:{refName:{include:["refs/heads/main"],exclude:[]}},
           bypassActors:{totalCount:1,pageInfo:{hasNextPage:false},nodes:[{bypassMode:"ALWAYS"}]},
           rules:{totalCount:1,pageInfo:{hasNextPage:false},nodes:[
             {type:"REQUIRED_LINEAR_HISTORY",parameters:null}]}},
          {name:"main-protection",enforcement:"ACTIVE",target:"BRANCH",
           conditions:{refName:{include:["refs/heads/main"],exclude:[]}},
           bypassActors:{totalCount:0,pageInfo:{hasNextPage:false},nodes:[]},
           rules:{totalCount:4,pageInfo:{hasNextPage:false},nodes:[
             {type:"REQUIRED_STATUS_CHECKS",parameters:{
               strictRequiredStatusChecksPolicy:true,doNotEnforceOnCreate:false,
               requiredStatusChecks:[
                 {context:"Rust CI / check"},
                 {context:"Frontend CI / check"},
                 {context:"Automation Policy / gate"}]}},
             {type:"PULL_REQUEST",parameters:{
               allowedMergeMethods:["MERGE","SQUASH","REBASE"],
               dismissStaleReviewsOnPush:true,requireCodeOwnerReview:false,
               requireLastPushApproval:false,requiredApprovingReviewCount:0,
               requiredReviewThreadResolution:true}},
             {type:"NON_FAST_FORWARD",parameters:null},
             {type:"DELETION",parameters:null}]}}
        ]
      },
      pullRequest:{
        number:$number,state:"OPEN",isDraft:false,mergeable:"MERGEABLE",mergeStateStatus:"CLEAN",
        headRefName:"automation/sync-upstream",headRefOid:$head,
        baseRefName:"main",baseRefOid:$base,headRepository:{nameWithOwner:$repo},
        autoMergeRequest:null,reviewDecision:null,
        reviewThreads:{nodes:[],pageInfo:{hasNextPage:false}}
      }
    }}}
  '
}

default_ruleset() {
  jq -nc --arg repo "${1:-owner/repo}" \
    '{id:21984329,name:"main-linear-history",source_type:"Repository",source:$repo,
      target:"branch",enforcement:"active",
      conditions:{ref_name:{include:["refs/heads/main"],exclude:[]}},
      rules:[{type:"required_linear_history"}],
      bypass_actors:[{actor_id:4667256,actor_type:"Integration",bypass_mode:"always"}]}'
}

merge_commit_fixture() {
  local sha="$1" base="$2" head="$3" source="$4" checkpoint="$5" verdict="${6:-eligible}"
  jq -nc --arg sha "${sha}" --arg base "${base}" --arg head "${head}" \
    --arg source "${source}" --arg checkpoint "${checkpoint}" --arg verdict "${verdict}" '
    {sha:$sha,parents:[{sha:$base},{sha:$head}],
     commit:{message:(
       "chore: sync " + $source + "\n\n" +
       "Sync-Upstream-Automation: true\n" +
       "Sync-Upstream-Source: " + $source + "\n" +
       "Sync-Upstream-Checkpoint: " + $checkpoint + "->" + ($source | split("@")[1]) + "\n" +
       "Sync-Upstream-Base: " + $base + "\n" +
       "Sync-Upstream-Policy-Verdict: " + $verdict)}}'
}

run_helper() {
  local fake_bin="$1" log="$2" head='' base='' source='' pr_number=0
  shift 2
  if [[ "$1" == merge && $# -eq 4 ]]; then
    set -- "$@" "${TEST_BASE_SHA}" "${TEST_SOURCE}" eligible
  fi
  if [[ "$1" == merge && $# -eq 7 ]]; then
    head="$4"
    base="$5"
    source="$6"
    if [[ "$3" =~ /pull/([1-9][0-9]*)/?$ ]]; then pr_number="${BASH_REMATCH[1]}"; else pr_number="$3"; fi
  fi
  preflight="${FAKE_GH_PREFLIGHT_RESPONSE:-}"
  if [[ -z "${preflight}" ]]; then
    preflight="$(jq -nc --argjson number "${pr_number}" --arg head "${head}" --arg base "${base}" --arg repo "${2:-}" \
      '{number:$number,state:"open",merged:false,draft:false,head:{sha:$head,ref:"automation/sync-upstream",repo:{full_name:$repo}},base:{ref:"main",sha:$base},auto_merge:null}')"
  fi
  head_commit="${FAKE_GH_HEAD_COMMIT_RESPONSE:-}"
  if [[ -z "${head_commit}" ]]; then
    message="$(printf 'Sync-Upstream-Automation: true\nSync-Upstream-Source: %s\nSync-Upstream-Checkpoint: %s->%s\nSync-Upstream-Base: %s\nSync-Upstream-Policy-Verdict: eligible' "${source}" "${TEST_CHECKPOINT_SHA}" "${source#*@}" "${base}")"
    head_commit="$(jq -nc --arg head "${head}" --arg base "${base}" --arg upstream "${source#*@}" --arg message "${message}" \
      '{sha:$head,parents:[{sha:$base},{sha:$upstream}],commit:{message:$message}}')"
  fi
  governance="${FAKE_GH_GOVERNANCE_RESPONSE:-}"
  if [[ -z "${governance}" ]]; then
    governance="$(default_governance "${pr_number}" "${head}" "${base}" "${2:-}")"
  fi
  ruleset="${FAKE_GH_RULESET_RESPONSE:-}"
  if [[ -z "${ruleset}" ]]; then
    ruleset="$(default_ruleset "${2:-}")"
  fi
  checks="${FAKE_GH_CHECKS_RESPONSE:-}"
  if [[ -z "${checks}" ]]; then
    checks="$(jq -nc --arg head "${head}" \
      '{total_count:3,check_runs:(["Automation Policy / gate","Frontend CI / check","Rust CI / check"] | map({id:1,name:.,head_sha:$head,status:"completed",conclusion:"success",app:{id:15368,slug:"github-actions"},check_suite:{id:1},details_url:"https://github.com/owner/repo/actions/runs/1"}))}')"
  fi
  PATH="${fake_bin}:${PATH}" FAKE_GH_LOG="${log}" \
    FAKE_GH_API_STATUS="${FAKE_GH_API_STATUS:-0}" \
    FAKE_GH_API_RESPONSE="${FAKE_GH_API_RESPONSE:-}" \
    FAKE_GH_PULL_RESPONSE="${FAKE_GH_PULL_RESPONSE:-}" \
    FAKE_GH_COMMIT_STATUS="${FAKE_GH_COMMIT_STATUS:-0}" \
    FAKE_GH_COMMIT_RESPONSE="${FAKE_GH_COMMIT_RESPONSE:-}" \
    FAKE_GH_PREFLIGHT_RESPONSE="${preflight}" \
    FAKE_GH_HEAD_COMMIT_RESPONSE="${head_commit}" \
    FAKE_GH_GOVERNANCE_RESPONSE="${governance}" \
    FAKE_GH_RULESET_RESPONSE="${ruleset}" \
    FAKE_GH_CHECKS_RESPONSE="${checks}" \
    FAKE_GH_REPOSITORY="${2:-}" FAKE_GH_EXPECTED_HEAD="${head}" \
    AERIS_CHECKS_GH_TOKEN="${AERIS_CHECKS_GH_TOKEN:-checks-read-token}" \
    "$HELPER" "$@"
}

test_merge_accepts_number_and_full_sha() {
  local bin="${RUN_ROOT}/merge-number/bin" log="${RUN_ROOT}/merge-number/gh.log" sha output
  sha='0123456789abcdef0123456789abcdef01234567'
  new_fake_gh "${bin}"
  FAKE_GH_API_RESPONSE='{"merged":true,"sha":"fedcba9876543210fedcba9876543210fedcba98"}' \
    FAKE_GH_PULL_RESPONSE="{\"number\":42,\"state\":\"closed\",\"merged\":true,\"merged_at\":\"2099-01-01T00:00:00Z\",\"draft\":false,\"head\":{\"sha\":\"${sha}\"},\"base\":{\"ref\":\"main\",\"sha\":\"abcdefabcdefabcdefabcdefabcdefabcdefabcd\"},\"auto_merge\":null,\"merged_by\":{\"login\":\"aeris-writer[bot]\"},\"merge_commit_sha\":\"fedcba9876543210fedcba9876543210fedcba98\"}" \
    FAKE_GH_COMMIT_RESPONSE="$(merge_commit_fixture fedcba9876543210fedcba9876543210fedcba98 "${TEST_BASE_SHA}" "${sha}" "${TEST_SOURCE}" "${TEST_CHECKPOINT_SHA}")" \
  output="$(run_helper "${bin}" "${log}" merge owner/repo 42 "${sha}")"
  assert_eq '' "${output}" 'merge should not emit output'
  [[ "$(grep -c '^api --method PUT ' "${log}")" -eq 1 ]] || fail 'merge mutation count'
  [[ "$(grep -c '^api repos/owner/repo/pulls/42 ' "${log}")" -eq 2 ]] || fail 'merge PR read count'
  [[ "$(grep -c '^api repos/owner/repo/rulesets/21984329 ' "${log}")" -eq 1 ]] || fail 'ruleset bypass proof read count'
  grep -Fq -- "-f sha=${sha}" "${log}" || fail 'merge head SHA argument'
  grep -Fq -- '-f merge_method=merge ' "${log}" || fail 'merge method must be a true merge'
  ! grep -Fq 'merge_method=squash' "${log}" || fail 'squash merge method must be gone'
  grep -Fq -- '-f commit_title=chore:\ sync\ upstream/example@1111111111111111111111111111111111111111' "${log}" ||
    fail 'merge commit title must name the synchronization source'
  grep -Fq 'Sync-Upstream-Source:' "${log}" || fail 'merge commit message must carry the source trailer'
  grep -Fq 'Sync-Upstream-Checkpoint:' "${log}" || fail 'merge commit message must carry the checkpoint trailer'
}

test_merge_accepts_matching_url() {
  local bin="${RUN_ROOT}/merge-url/bin" log="${RUN_ROOT}/merge-url/gh.log" sha
  sha='0123456789abcdef0123456789abcdef01234567'
  new_fake_gh "${bin}"
  FAKE_GH_API_RESPONSE='{"merged":true,"sha":"fedcba9876543210fedcba9876543210fedcba98"}' \
    FAKE_GH_PULL_RESPONSE="{\"number\":7,\"state\":\"closed\",\"merged\":true,\"merged_at\":\"2099-01-01T00:00:00Z\",\"draft\":false,\"head\":{\"sha\":\"${sha}\"},\"base\":{\"ref\":\"main\",\"sha\":\"abcdefabcdefabcdefabcdefabcdefabcdefabcd\"},\"auto_merge\":null,\"merged_by\":{\"login\":\"aeris-writer[bot]\"},\"merge_commit_sha\":\"fedcba9876543210fedcba9876543210fedcba98\"}" \
    FAKE_GH_COMMIT_RESPONSE="$(merge_commit_fixture fedcba9876543210fedcba9876543210fedcba98 "${TEST_BASE_SHA}" "${sha}" "${TEST_SOURCE}" "${TEST_CHECKPOINT_SHA}")" \
    run_helper "${bin}" "${log}" merge owner/repo https://github.com/owner/repo/pull/7/ "${sha}"
  [[ "$(grep -c '^api --method PUT ' "${log}")" -eq 1 ]] || fail 'URL merge mutation count'
  [[ "$(grep -c '^api repos/owner/repo/pulls/7 ' "${log}")" -eq 2 ]] || fail 'URL merge PR read count'
}

test_merge_rejects_unproven_response() {
  local bin="${RUN_ROOT}/merge-unproven/bin" log="${RUN_ROOT}/merge-unproven/gh.log" status
  local sha='0123456789abcdef0123456789abcdef01234567'
  new_fake_gh "${bin}"
  set +e
  FAKE_GH_API_RESPONSE='{"merged":true,"sha":"wrong"}' \
    FAKE_GH_PULL_RESPONSE='{"number":42,"state":"open"}' \
    run_helper "${bin}" "${log}" merge owner/repo 42 "${sha}" >/dev/null 2>&1
  status=$?
  set -e
  assert_status 64 "${status}" 'unproven merge response must fail closed'
  [[ "$(grep -c '^api --method PUT ' "${log}")" -eq 1 ]] || fail 'unproven response retried mutation'
  [[ "$(grep -c '^api repos/owner/repo/pulls/42 ' "${log}")" -eq 2 ]] || fail 'unproven response PR reads'
}

test_merge_accepts_lost_response_when_readback_is_exact() {
  local bin="${RUN_ROOT}/merge-lost/bin" log="${RUN_ROOT}/merge-lost/gh.log" sha
  sha='0123456789abcdef0123456789abcdef01234567'
  new_fake_gh "${bin}"
  FAKE_GH_API_RESPONSE='not-json' \
    FAKE_GH_PULL_RESPONSE="{\"number\":42,\"state\":\"closed\",\"merged\":true,\"merged_at\":\"2099-01-01T00:00:00Z\",\"draft\":false,\"head\":{\"sha\":\"${sha}\"},\"base\":{\"ref\":\"main\",\"sha\":\"abcdefabcdefabcdefabcdefabcdefabcdefabcd\"},\"auto_merge\":null,\"merged_by\":{\"login\":\"aeris-writer[bot]\"},\"merge_commit_sha\":\"fedcba9876543210fedcba9876543210fedcba98\"}" \
    FAKE_GH_COMMIT_RESPONSE="$(merge_commit_fixture fedcba9876543210fedcba9876543210fedcba98 "${TEST_BASE_SHA}" "${sha}" "${TEST_SOURCE}" "${TEST_CHECKPOINT_SHA}")" \
    run_helper "${bin}" "${log}" merge owner/repo 42 "${sha}"
  [[ "$(grep -c '^api ' "${log}")" -eq 8 ]] || fail 'lost response did not perform checks, governance, one mutation, and readbacks'
}

test_merge_failed_mutation_open_readback_fails() {
  local bin="${RUN_ROOT}/merge-failed-open/bin" log="${RUN_ROOT}/merge-failed-open/gh.log" sha status
  sha='0123456789abcdef0123456789abcdef01234567'
  new_fake_gh "${bin}"
  set +e
  FAKE_GH_API_STATUS=17 FAKE_GH_PULL_RESPONSE='{"number":42,"state":"open","head":{"sha":"0123456789abcdef0123456789abcdef01234567"}}' \
    run_helper "${bin}" "${log}" merge owner/repo 42 "${sha}" >/dev/null 2>&1
  status=$?
  set -e
  assert_status 64 "${status}" 'failed mutation with open readback must fail closed'
  [[ "$(grep -c '^api --method PUT ' "${log}")" -eq 1 ]] || fail 'failed mutation retried'
  [[ "$(grep -c '^api repos/owner/repo/pulls/42 ' "${log}")" -eq 2 ]] ||
    fail 'failed mutation did not perform preflight and one pull readback'
}

test_merge_commit_readback_failure_fails_closed() {
  local bin="${RUN_ROOT}/merge-commit-failed/bin" log="${RUN_ROOT}/merge-commit-failed/gh.log" status
  local sha='0123456789abcdef0123456789abcdef01234567'
  new_fake_gh "${bin}"
  set +e
  FAKE_GH_API_RESPONSE='{"merged":true,"sha":"fedcba9876543210fedcba9876543210fedcba98"}' \
    FAKE_GH_PULL_RESPONSE="{\"number\":42,\"state\":\"closed\",\"merged\":true,\"merged_at\":\"2099-01-01T00:00:00Z\",\"draft\":false,\"head\":{\"sha\":\"${sha}\"},\"base\":{\"ref\":\"main\",\"sha\":\"abcdefabcdefabcdefabcdefabcdefabcdefabcd\"},\"auto_merge\":null,\"merged_by\":{\"login\":\"aeris-writer[bot]\"},\"merge_commit_sha\":\"fedcba9876543210fedcba9876543210fedcba98\"}" \
    FAKE_GH_COMMIT_STATUS=29 \
    run_helper "${bin}" "${log}" merge owner/repo 42 "${sha}" >/dev/null 2>&1
  status=$?
  set -e
  assert_status 64 "${status}" 'failed commit readback must fail closed'
  [[ "$(grep -c '^api --method PUT ' "${log}")" -eq 1 ]] || fail 'commit readback failure retried mutation'
  [[ "$(grep -c '^api repos/owner/repo/pulls/42 ' "${log}")" -eq 2 ]] ||
    fail 'commit readback failure did not perform preflight and one pull readback'
  [[ "$(grep -c '^api repos/owner/repo/commits/fedcba9876543210fedcba9876543210fedcba98 ' "${log}")" -eq 1 ]] ||
    fail 'commit readback failure did not perform exactly one commit readback'
}

test_invalid_input_never_calls_gh() {
  local bin="${RUN_ROOT}/invalid/bin" log="${RUN_ROOT}/invalid/gh.log" status
  new_fake_gh "${bin}"
  set +e
  run_helper "${bin}" "${log}" merge owner/repo https://github.com/other/repo/pull/7 abc >/dev/null 2>&1
  status=$?
  set -e
  [[ ! -e "${log}" ]] || fail 'invalid PR URL called gh'
  assert_status 64 "${status}" 'invalid PR URL status'

  set +e
  run_helper "${bin}" "${log}" merge owner/repo 7 deadbeef >/dev/null 2>&1
  status=$?
  set -e
  [[ ! -e "${log}" ]] || fail 'short SHA called gh'
  assert_status 64 "${status}" 'short SHA status'
}

test_api_failure_with_open_readback_fails_closed() {
  local bin="${RUN_ROOT}/api-failure/bin" log="${RUN_ROOT}/api-failure/gh.log" status
  new_fake_gh "${bin}"
  set +e
  FAKE_GH_API_STATUS=23 \
    FAKE_GH_PULL_RESPONSE='{"number":7,"state":"open","head":{"sha":"0123456789abcdef0123456789abcdef01234567"}}' \
    run_helper "${bin}" "${log}" merge owner/repo 7 0123456789abcdef0123456789abcdef01234567 >/dev/null 2>&1
  status=$?
  set -e
  assert_status 64 "${status}" 'API failure with open readback must fail closed'
  [[ "$(grep -c '^api --method PUT ' "${log}")" -eq 1 ]] || fail 'API failure retried mutation'
  [[ "$(grep -c '^api repos/owner/repo/pulls/7 ' "${log}")" -eq 2 ]] ||
    fail 'API failure did not perform preflight and one pull readback'
}

test_merge_preflight_drift_never_mutates() {
  local bin="${RUN_ROOT}/preflight-drift/bin" log="${RUN_ROOT}/preflight-drift/gh.log" status
  local sha='0123456789abcdef0123456789abcdef01234567'
  new_fake_gh "${bin}"
  set +e
  FAKE_GH_PREFLIGHT_RESPONSE="{\"number\":42,\"state\":\"open\",\"merged\":false,\"draft\":false,\"head\":{\"sha\":\"${sha}\",\"ref\":\"automation/sync-upstream\",\"repo\":{\"full_name\":\"owner/repo\"}},\"base\":{\"ref\":\"main\",\"sha\":\"2222222222222222222222222222222222222222\"},\"auto_merge\":null}" \
    run_helper "${bin}" "${log}" merge owner/repo 42 "${sha}" >/dev/null 2>&1
  status=$?
  set -e
  assert_status 64 "${status}" 'base drift must fail closed before mutation'
  [[ "$(grep -c '^api --method PUT ' "${log}" || true)" -eq 0 ]] || fail 'base drift reached mutation'
}

test_merge_governance_and_trailer_failures_never_mutate() {
  local case_name bin log status sha='0123456789abcdef0123456789abcdef01234567'
  for case_name in unresolved blocking-review bad-trailer bad-parent; do
    bin="${RUN_ROOT}/${case_name}/bin"
    log="${RUN_ROOT}/${case_name}/gh.log"
    new_fake_gh "${bin}"
    set +e
    if [[ "${case_name}" == unresolved ]]; then
      FAKE_GH_GOVERNANCE_RESPONSE="{\"data\":{\"repository\":{\"pullRequest\":{\"number\":42,\"state\":\"OPEN\",\"isDraft\":false,\"headRefName\":\"automation/sync-upstream\",\"headRefOid\":\"${sha}\",\"baseRefName\":\"main\",\"baseRefOid\":\"${TEST_BASE_SHA}\",\"headRepository\":{\"nameWithOwner\":\"owner/repo\"},\"autoMergeRequest\":null,\"reviewDecision\":null,\"reviewThreads\":{\"nodes\":[{\"isResolved\":false}],\"pageInfo\":{\"hasNextPage\":false}}}}}}" \
        run_helper "${bin}" "${log}" merge owner/repo 42 "${sha}" >/dev/null 2>&1
    elif [[ "${case_name}" == blocking-review ]]; then
      FAKE_GH_GOVERNANCE_RESPONSE="{\"data\":{\"repository\":{\"pullRequest\":{\"number\":42,\"state\":\"OPEN\",\"isDraft\":false,\"headRefName\":\"automation/sync-upstream\",\"headRefOid\":\"${sha}\",\"baseRefName\":\"main\",\"baseRefOid\":\"${TEST_BASE_SHA}\",\"headRepository\":{\"nameWithOwner\":\"owner/repo\"},\"autoMergeRequest\":null,\"reviewDecision\":\"CHANGES_REQUESTED\",\"reviewThreads\":{\"nodes\":[],\"pageInfo\":{\"hasNextPage\":false}}}}}}" \
        run_helper "${bin}" "${log}" merge owner/repo 42 "${sha}" >/dev/null 2>&1
    elif [[ "${case_name}" == bad-trailer ]]; then
      FAKE_GH_HEAD_COMMIT_RESPONSE="{\"sha\":\"${sha}\",\"parents\":[{\"sha\":\"${TEST_BASE_SHA}\"},{\"sha\":\"1111111111111111111111111111111111111111\"}],\"commit\":{\"message\":\"Sync-Upstream-Automation: true\\nSync-Upstream-Source: attacker/repo@1111111111111111111111111111111111111111\\nSync-Upstream-Base: ${TEST_BASE_SHA}\\nSync-Upstream-Policy-Verdict: eligible\"}}" \
        run_helper "${bin}" "${log}" merge owner/repo 42 "${sha}" >/dev/null 2>&1
    else
      FAKE_GH_HEAD_COMMIT_RESPONSE="{\"sha\":\"${sha}\",\"parents\":[{\"sha\":\"${TEST_BASE_SHA}\"},{\"sha\":\"2222222222222222222222222222222222222222\"}],\"commit\":{\"message\":\"Sync-Upstream-Automation: true\\nSync-Upstream-Source: ${TEST_SOURCE}\\nSync-Upstream-Base: ${TEST_BASE_SHA}\\nSync-Upstream-Policy-Verdict: eligible\"}}" \
        run_helper "${bin}" "${log}" merge owner/repo 42 "${sha}" >/dev/null 2>&1
    fi
    status=$?
    set -e
    assert_status 64 "${status}" "${case_name} must fail closed before mutation"
    [[ "$(grep -c '^api --method PUT ' "${log}" || true)" -eq 0 ]] || fail "${case_name} reached mutation"
  done
}

test_manual_verdict_never_calls_gh() {
  local bin="${RUN_ROOT}/manual-verdict/bin" log="${RUN_ROOT}/manual-verdict/gh.log" status
  new_fake_gh "${bin}"
  set +e
  run_helper "${bin}" "${log}" merge owner/repo 42 \
    0123456789abcdef0123456789abcdef01234567 "${TEST_BASE_SHA}" "${TEST_SOURCE}" manual_review \
    >/dev/null 2>&1
  status=$?
  set -e
  assert_status 64 "${status}" 'manual verdict must reject direct merge'
  [[ ! -e "${log}" ]] || fail 'manual verdict called gh'
}

test_conflict_verdict_requires_attestation_before_gh() {
  local bin="${RUN_ROOT}/conflict-no-attestation/bin" log="${RUN_ROOT}/conflict-no-attestation/gh.log" status
  new_fake_gh "${bin}"
  set +e
  run_helper "${bin}" "${log}" merge owner/repo 42 \
    0123456789abcdef0123456789abcdef01234567 "${TEST_BASE_SHA}" "${TEST_SOURCE}" conflict_ai_review \
    >/dev/null 2>&1
  status=$?
  set -e
  assert_status 64 "${status}" 'AI conflict verdict without attestation must fail closed'
  [[ ! -e "${log}" ]] || fail 'missing conflict attestation called gh'
}

test_conflict_verdict_requires_full_artifact_chain_before_gh() {
  local bin="${RUN_ROOT}/conflict-no-artifacts/bin" log="${RUN_ROOT}/conflict-no-artifacts/gh.log" status
  new_fake_gh "${bin}"
  set +e
  run_helper "${bin}" "${log}" merge owner/repo 42 \
    0123456789abcdef0123456789abcdef01234567 "${TEST_BASE_SHA}" "${TEST_SOURCE}" conflict_ai_review \
    "${RUN_ROOT}/attestation.json" 0000000000000000000000000000000000000000000000000000000000000000 \
    >/dev/null 2>&1
  status=$?
  set -e
  assert_status 64 "${status}" 'AI conflict verdict without all artifact paths and hashes must fail closed'
  [[ ! -e "${log}" ]] || fail 'missing conflict artifact chain called gh'
}

test_unsuccessful_exact_head_check_never_mutates() {
  local bin="${RUN_ROOT}/failed-check/bin" log="${RUN_ROOT}/failed-check/gh.log" status
  local sha='0123456789abcdef0123456789abcdef01234567'
  new_fake_gh "${bin}"
  set +e
  FAKE_GH_CHECKS_RESPONSE="$(jq -nc --arg head "${sha}" \
    '{total_count:3,check_runs:(["Automation Policy / gate","Frontend CI / check","Rust CI / check"] | map({id:1,name:.,head_sha:$head,status:"completed",conclusion:(if . == "Rust CI / check" then "failure" else "success" end),app:{id:15368,slug:"github-actions"},check_suite:{id:1},details_url:"https://github.com/owner/repo/actions/runs/1"}))}')" \
    run_helper "${bin}" "${log}" merge owner/repo 42 "${sha}" >/dev/null 2>&1
  status=$?
  set -e
  assert_status 64 "${status}" 'failed exact-head check must fail closed'
  [[ "$(grep -c '^api --method PUT ' "${log}" || true)" -eq 0 ]] || fail 'failed check reached mutation'
}

test_branch_protection_drift_never_mutates() {
  local name filter bin log governance status
  local sha='0123456789abcdef0123456789abcdef01234567'
  local -a cases=(
    'strict:.data.repository.rulesets.nodes[] |= (if .name == "main-protection" then .rules.nodes[] |= (if .type == "REQUIRED_STATUS_CHECKS" then .parameters.strictRequiredStatusChecksPolicy = false else . end) else . end)'
    'bypass:.data.repository.rulesets.nodes[] |= (if .name == "main-protection" then .bypassActors = {totalCount:1,pageInfo:{hasNextPage:false},nodes:[{bypassMode:"ALWAYS"}]} else . end)'
    'context:.data.repository.rulesets.nodes[] |= (if .name == "main-protection" then .rules.nodes[] |= (if .type == "REQUIRED_STATUS_CHECKS" then .parameters.requiredStatusChecks[0].context = "Unexpected / check" else . end) else . end)'
    'linear-bypass:.data.repository.rulesets.nodes[] |= (if .name == "main-linear-history" then .bypassActors = {totalCount:0,pageInfo:{hasNextPage:false},nodes:[]} else . end)'
    'evaluate:.data.repository.rulesets.nodes[0].enforcement = "EVALUATE"'
    'merge-flag:.data.repository.mergeCommitAllowed = false'
  )
  for entry in "${cases[@]}"; do
    name="${entry%%:*}"
    filter="${entry#*:}"
    bin="${RUN_ROOT}/protection-${name}/bin"
    log="${RUN_ROOT}/protection-${name}/gh.log"
    new_fake_gh "${bin}"
    governance="$(default_governance 42 "${sha}" "${TEST_BASE_SHA}" owner/repo | jq -c "${filter}")"
    set +e
    FAKE_GH_GOVERNANCE_RESPONSE="${governance}" \
      run_helper "${bin}" "${log}" merge owner/repo 42 "${sha}" >/dev/null 2>&1
    status=$?
    set -e
    assert_status 64 "${status}" "${name} governance drift must fail closed"
    [[ "$(grep -c '^api --method PUT ' "${log}" || true)" -eq 0 ]] ||
      fail "${name} governance drift reached mutation"
  done
}

test_ruleset_bypass_drift_never_mutates() {
  local name filter bin log ruleset status
  local sha='0123456789abcdef0123456789abcdef01234567'
  local -a cases=(
    'wrong-actor:.bypass_actors[0].actor_id = 9999999'
    'two-actors:.bypass_actors += [{actor_id:9999999,actor_type:"Integration",bypass_mode:"always"}]'
    'evaluate:.enforcement = "evaluate"'
  )
  for entry in "${cases[@]}"; do
    name="${entry%%:*}"
    filter="${entry#*:}"
    bin="${RUN_ROOT}/ruleset-${name}/bin"
    log="${RUN_ROOT}/ruleset-${name}/gh.log"
    new_fake_gh "${bin}"
    ruleset="$(default_ruleset owner/repo | jq -c "${filter}")"
    set +e
    FAKE_GH_RULESET_RESPONSE="${ruleset}" \
      run_helper "${bin}" "${log}" merge owner/repo 42 "${sha}" >/dev/null 2>&1
    status=$?
    set -e
    assert_status 64 "${status}" "${name} bypass drift must fail closed"
    [[ "$(grep -c '^api --method PUT ' "${log}" || true)" -eq 0 ]] ||
      fail "${name} bypass drift reached mutation"
  done
}

test_merge_commit_shape_failures_fail_closed() {
  local name bin log commit_response status merge_sha
  local sha='0123456789abcdef0123456789abcdef01234567'
  merge_sha='fedcba9876543210fedcba9876543210fedcba98'
  for name in single-parent wrong-second-parent missing-trailer; do
    bin="${RUN_ROOT}/landing-${name}/bin"
    log="${RUN_ROOT}/landing-${name}/gh.log"
    new_fake_gh "${bin}"
    case "${name}" in
      single-parent)
        commit_response="$(merge_commit_fixture "${merge_sha}" "${TEST_BASE_SHA}" "${sha}" "${TEST_SOURCE}" "${TEST_CHECKPOINT_SHA}" |
          jq -c '.parents = [.parents[0]]')"
        ;;
      wrong-second-parent)
        commit_response="$(merge_commit_fixture "${merge_sha}" "${TEST_BASE_SHA}" "${sha}" "${TEST_SOURCE}" "${TEST_CHECKPOINT_SHA}" |
          jq -c '.parents[1].sha = "2222222222222222222222222222222222222222"')"
        ;;
      missing-trailer)
        commit_response="$(merge_commit_fixture "${merge_sha}" "${TEST_BASE_SHA}" "${sha}" "${TEST_SOURCE}" "${TEST_CHECKPOINT_SHA}" |
          jq -c '.commit.message |= (split("\n") | map(select(startswith("Sync-Upstream-Checkpoint: ") | not)) | join("\n"))')"
        ;;
    esac
    set +e
    FAKE_GH_API_RESPONSE="{\"merged\":true,\"sha\":\"${merge_sha}\"}" \
      FAKE_GH_PULL_RESPONSE="{\"number\":42,\"state\":\"closed\",\"merged\":true,\"merged_at\":\"2099-01-01T00:00:00Z\",\"draft\":false,\"head\":{\"sha\":\"${sha}\"},\"base\":{\"ref\":\"main\",\"sha\":\"abcdefabcdefabcdefabcdefabcdefabcdefabcd\"},\"auto_merge\":null,\"merged_by\":{\"login\":\"aeris-writer[bot]\"},\"merge_commit_sha\":\"${merge_sha}\"}" \
      FAKE_GH_COMMIT_RESPONSE="${commit_response}" \
      run_helper "${bin}" "${log}" merge owner/repo 42 "${sha}" >/dev/null 2>&1
    status=$?
    set -e
    assert_status 64 "${status}" "${name} merge landing must fail closed"
    [[ "$(grep -c '^api --method PUT ' "${log}")" -eq 1 ]] || fail "${name} changed the mutation count"
    [[ "$(grep -c "^api repos/owner/repo/commits/${merge_sha} " "${log}")" -eq 1 ]] ||
      fail "${name} did not perform the merge commit readback"
  done
}

test_legacy_arm_action_is_rejected() {
  local bin="${RUN_ROOT}/legacy-arm/bin" log="${RUN_ROOT}/legacy-arm/gh.log" status
  new_fake_gh "${bin}"
  set +e
  run_helper "${bin}" "${log}" arm owner/repo 7 0123456789abcdef0123456789abcdef01234567 >/dev/null 2>&1
  status=$?
  set -e
  assert_status 64 "${status}" 'legacy arm action must be rejected'
  [[ ! -e "${log}" ]] || fail 'legacy arm action invoked gh'
}

test_disarm_when_enabled() {
  local bin="${RUN_ROOT}/disarm-enabled/bin" log="${RUN_ROOT}/disarm-enabled/gh.log"
  new_fake_gh "${bin}"
  PATH="${bin}:${PATH}" FAKE_GH_LOG="${log}" FAKE_GH_AUTO_MERGE=true \
    "$HELPER" disarm owner/repo 9
  assert_eq $'pr view 9 --repo owner/repo --json autoMergeRequest --jq .autoMergeRequest\\ \\!=\\ null \npr merge 9 --repo owner/repo --disable-auto ' \
    "$(<"${log}")" 'enabled disarm arguments'
}

test_disarm_is_noop_when_disabled() {
  local bin="${RUN_ROOT}/disarm-disabled/bin" log="${RUN_ROOT}/disarm-disabled/gh.log"
  new_fake_gh "${bin}"
  PATH="${bin}:${PATH}" FAKE_GH_LOG="${log}" FAKE_GH_AUTO_MERGE=false \
    "$HELPER" disarm owner/repo 9
  assert_eq 'pr view 9 --repo owner/repo --json autoMergeRequest --jq .autoMergeRequest\ \!=\ null ' \
    "$(<"${log}")" 'disabled disarm must not merge'
}

test_disarm_fails_closed_on_unknown_response() {
  local bin="${RUN_ROOT}/disarm-unknown/bin" log="${RUN_ROOT}/disarm-unknown/gh.log" status
  new_fake_gh "${bin}"
  set +e
  PATH="${bin}:${PATH}" FAKE_GH_LOG="${log}" FAKE_GH_AUTO_MERGE=unknown \
    "$HELPER" disarm owner/repo 9 >/dev/null 2>&1
  status=$?
  set -e
  assert_status 64 "${status}" 'unknown auto-merge state must fail closed'
  assert_eq 'pr view 9 --repo owner/repo --json autoMergeRequest --jq .autoMergeRequest\ \!=\ null ' \
    "$(<"${log}")" 'unknown state must not disable auto merge'
}

test_disarm_propagates_query_error() {
  local bin="${RUN_ROOT}/disarm-query-error/bin" log="${RUN_ROOT}/disarm-query-error/gh.log" status
  new_fake_gh "${bin}"
  set +e
  PATH="${bin}:${PATH}" FAKE_GH_LOG="${log}" FAKE_GH_STATUS=31 \
    "$HELPER" disarm owner/repo 9 >/dev/null 2>&1
  status=$?
  set -e
  assert_status 31 "${status}" 'auto-merge query error status'
  assert_eq 'pr view 9 --repo owner/repo --json autoMergeRequest --jq .autoMergeRequest\ \!=\ null ' \
    "$(<"${log}")" 'query failure must not disable auto merge'
}

test_disarm_propagates_disable_error() {
  local bin="${RUN_ROOT}/disarm-disable-error/bin" log="${RUN_ROOT}/disarm-disable-error/gh.log" status
  new_fake_gh "${bin}"
  set +e
  PATH="${bin}:${PATH}" FAKE_GH_LOG="${log}" FAKE_GH_AUTO_MERGE=true FAKE_GH_MERGE_STATUS=37 \
    "$HELPER" disarm owner/repo 9 >/dev/null 2>&1
  status=$?
  set -e
  assert_status 37 "${status}" 'disable auto-merge error status'
  assert_eq $'pr view 9 --repo owner/repo --json autoMergeRequest --jq .autoMergeRequest\\ \\!=\\ null \npr merge 9 --repo owner/repo --disable-auto ' \
    "$(<"${log}")" 'disable failure arguments'
}

test_disarm_blocks_mutation_when_expiry_crosses_after_read() {
  local bin="${RUN_ROOT}/disarm-expiry/bin" log="${RUN_ROOT}/disarm-expiry/gh.log"
  local clock_calls="${RUN_ROOT}/disarm-expiry/clock.log" status
  new_fake_gh "${bin}"
  new_expiring_clock "${bin}" "${clock_calls}"
  set +e
  PATH="${bin}:${PATH}" FAKE_GH_LOG="${log}" FAKE_GH_AUTO_MERGE=true \
    CLOCK_CALLS="${clock_calls}" AERIS_AUTONOMY_EXPIRES_AT='2033-05-18T03:33:20Z' \
    "$HELPER" disarm owner/repo 9 >/dev/null 2>&1
  status=$?
  set -e
  assert_status 78 "${status}" 'expiry after auto-merge read must fail closed'
  assert_eq 'pr view 9 --repo owner/repo --json autoMergeRequest --jq .autoMergeRequest\ \!=\ null ' \
    "$(<"${log}")" 'expired disarm must not reach the mutation'
}

test_merge_accepts_number_and_full_sha
test_merge_accepts_matching_url
test_merge_rejects_unproven_response
test_merge_accepts_lost_response_when_readback_is_exact
test_merge_failed_mutation_open_readback_fails
test_merge_commit_readback_failure_fails_closed
test_invalid_input_never_calls_gh
test_api_failure_with_open_readback_fails_closed
test_merge_preflight_drift_never_mutates
test_merge_governance_and_trailer_failures_never_mutate
test_manual_verdict_never_calls_gh
test_conflict_verdict_requires_attestation_before_gh
test_conflict_verdict_requires_full_artifact_chain_before_gh
test_unsuccessful_exact_head_check_never_mutates
test_branch_protection_drift_never_mutates
test_ruleset_bypass_drift_never_mutates
test_merge_commit_shape_failures_fail_closed
test_legacy_arm_action_is_rejected
test_disarm_when_enabled
test_disarm_is_noop_when_disabled
test_disarm_fails_closed_on_unknown_response
test_disarm_propagates_query_error
test_disarm_propagates_disable_error
test_disarm_blocks_mutation_when_expiry_crosses_after_read

printf 'PASS manage sync automerge (%s)\n' "${RUN_ROOT}"
