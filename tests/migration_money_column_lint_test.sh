#!/usr/bin/env bash
# 迁移 lint：金额语义列禁止使用 double precision / float8。
# 新增迁移文件一律受检；存量含 float8 金额列的文件登记在豁免清单里
# （冻结不回改，见 #559），清单只减不增。
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MIGRATIONS="${REPO_ROOT}/crates/aether-data/adapters/postgres/migrations"

[[ -d "${MIGRATIONS}" ]] || {
  printf 'missing migrations directory: %s\n' "${MIGRATIONS}" >&2
  exit 1
}

# 存量豁免：这些文件里的 float8 金额列是已决策冻结的历史债务。
EXEMPT_FILES=(
  "20260403000000_baseline.sql"
  "20260518000000_add_usage_counter_deltas.sql"
  "20260903010000_add_daily_usage_limits.sql"
  "20260913010000_add_request_fund_reservations.sql"
  "20260923000000_add_provider_cost_catalogs.sql"
)

is_exempt() {
  local name="$1"
  local exempt
  for exempt in "${EXEMPT_FILES[@]}"; do
    [[ "${name}" == "${exempt}" ]] && return 0
  done
  return 1
}

# 金额语义列名（cost/amount/balance/price/fee/usd/recharge/gift）后
# 直接声明 double precision / float8 即违规；numeric(20,8) 列带
# '0'::double precision 默认值不算违规（声明类型是 numeric）。
violations=0
while IFS=: read -r file line text; do
  name="$(basename "${file}")"
  if is_exempt "${name}"; then
    continue
  fi
  printf 'money column must not be double precision: %s:%s: %s\n' \
    "${file#${REPO_ROOT}/}" "${line}" "${text}" >&2
  violations=$((violations + 1))
done < <(grep -nE "(cost|amount|balance|price|fee|usd|recharge|gift)[a-z0-9_]* +(double precision|float8)" "${MIGRATIONS}"/*.sql || true)

if [[ "${violations}" -gt 0 ]]; then
  printf 'migration money column lint: %d violation(s)\n' "${violations}" >&2
  exit 1
fi

printf 'ok migration money column lint\n'
