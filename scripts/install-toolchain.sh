#!/usr/bin/env bash
set -euo pipefail

PLATFORM=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m | sed 's/aarch64/arm64/' | sed 's/x86_64/amd64/')

mkdir -p "${CHART_TMP_DIR}"

## Install kubeconform
mkdir -p "${CHART_TMP_DIR}/kubeconform"
curl -sSL "https://github.com/yannh/kubeconform/releases/download/${KUBECONFORM_VERSION}/kubeconform-${PLATFORM}-${ARCH}.tar.gz" | tar xz -C "${CHART_TMP_DIR}/kubeconform"
mv "${CHART_TMP_DIR}/kubeconform/kubeconform" "${CHART_TOOLS_DIR}/kubeconform"

## Install helm v3
mkdir -p "${CHART_TMP_DIR}/helmv3"
curl -sSL "https://get.helm.sh/helm-${HELMV3_VERSION}-${PLATFORM}-${ARCH}.tar.gz" | tar xz -C "${CHART_TMP_DIR}/helmv3"
mv "${CHART_TMP_DIR}/helmv3/${PLATFORM}-${ARCH}/helm" "${CHART_TOOLS_DIR}/helmv3"

rm -rf "${CHART_TMP_DIR}"
