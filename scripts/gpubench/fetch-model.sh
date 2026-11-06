#!/usr/bin/env bash
# Download a model from models.json into the model cache and verify it.
#
#   fetch-model.sh NAME     prints the path of the verified file

set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/lib/common.sh"

name="${1:?usage: fetch-model.sh NAME (see models.json)}"
manifest="$gpubench_dir/models.json"
jq -e --arg n "$name" 'has($n)' "$manifest" >/dev/null || die "no model '$name' in models.json"
field() { jq -r --arg n "$name" --arg f "$1" '.[$n][$f]' "$manifest"; }

dest="$model_dir/$(field file)"
url="https://huggingface.co/$(field repo)/resolve/$(field revision)/$(field file)"
fetch_file "$url" "$(field sha256)" "$dest"
echo "$dest"
