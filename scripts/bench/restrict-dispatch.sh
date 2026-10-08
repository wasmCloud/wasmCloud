#!/usr/bin/env bash
set -euo pipefail

# Restrict bench workflow actors at the repository policy layer. Run with a
# GitHub token that has Administration: write on wasmCloud/wasmCloud.

repo=wasmCloud/wasmCloud
policy_name='Bench workflows: maintainers only'
api_version='X-GitHub-Api-Version: 2026-03-10'

need_id() {
  local id="$1" label="$2"
  [[ "$id" =~ ^[0-9]+$ ]] || { echo "could not resolve $label" >&2; exit 1; }
  printf '%s' "$id"
}

ci_id="$(need_id "$(gh api orgs/wasmCloud/teams/ci-maintainers --jq .id)" ci-maintainers)"
org_id="$(need_id "$(gh api orgs/wasmCloud/teams/org-maintainers --jq .id)" org-maintainers)"
# The release workflow publishes releases using this account's PAT.
release_id="$(need_id "$(gh api users/automation-wasmcloud --jq .id)" automation-wasmcloud)"

policy_id="$(gh api -H "$api_version" "repos/$repo/actions/policies?has_parents=false" |
  jq -r --arg name "$policy_name" '.policies[] | select(.name == $name) | .id')"

payload="$(mktemp)"
trap 'rm -f "$payload"' EXIT
jq -n \
  --arg name "$policy_name" \
  --argjson ci "$ci_id" \
  --argjson org "$org_id" \
  --argjson release "$release_id" \
  '{
    name: $name,
    enforcement: "active",
    conditions: {
      workflow_path: {
        include: [
          ".github/workflows/bench.yml",
          ".github/workflows/bench-run.yml",
          ".github/workflows/bench-compare.yml",
          ".github/workflows/k6bench.yml",
          ".github/workflows/k6bench-run.yml"
        ],
        exclude: []
      }
    },
    rules: [{
      type: "restrict_actions_actors",
      parameters: {
        allowed_actors: [
          {id: $ci, type: "Team"},
          {id: $org, type: "Team"},
          {id: $release, type: "User"}
        ]
      }
    }]
  }' >"$payload"

if [ -n "$policy_id" ]; then
  gh api -H "$api_version" -X PUT "repos/$repo/actions/policies/$policy_id" --input "$payload" >/dev/null
else
  policy_id="$(gh api -H "$api_version" -X POST "repos/$repo/actions/policies" --input "$payload" --jq .id)"
fi

gh api -H "$api_version" "repos/$repo/actions/policies/$policy_id" \
  --jq '{name, enforcement, conditions, rules}'
