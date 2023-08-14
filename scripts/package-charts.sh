#!/usr/bin/env bash
set -euo pipefail

helmv3 package "${CHARTS_DIR}/"* \
    --destination "${CHART_BUILD_DIR}/charts" \
    --dependency-update
