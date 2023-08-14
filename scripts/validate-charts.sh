#!/usr/bin/env bash
set -euo pipefail

FAILED_V3=()

cd "${CHARTS_DIR}"
for d in */; do
    EXTRA_ARGS=""
    if [ -f "${CHARTS_DIR}/${d}/ci/extra_args" ]; then
        EXTRA_ARGS=$(cat "${CHARTS_DIR}/${d}/ci/extra_args")
    fi
    echo "Validating chart ${d} w/ helm v3"
    helmv3 template "${CHARTS_DIR}/${d}" $EXTRA_ARGS | kubeconform -strict -ignore-missing-schemas || FAILED_V3+=("${d}")
done

if [[ "${#FAILED_V3[@]}" -eq 0 ]]; then
    echo "All charts passed validations!"
    exit 0
else
    echo "Helm v3:"
    for chart in "${FAILED_V3[@]}"; do
        printf "%40s ❌\n" "$chart"
    done
    exit 1
fi
