#!/usr/bin/env bash
set -euo pipefail

FAILED_V3=()

cd "${CHARTS_DIR}"
for d in */; do
    echo "Linting chart ${d} w/ helm v3"
    helmv3 lint "${CHARTS_DIR}/${d}" || FAILED_V3+=("${d}")
done

if [[ "${#FAILED_V3[@]}" -eq 0 ]]; then
    echo "All charts passed linting!"
    exit 0
else
    echo "Helm v3:"
    for chart in "${FAILED_V3[@]}"; do
        printf "%40s ❌\n" "$chart"
    done
    exit 1
fi
