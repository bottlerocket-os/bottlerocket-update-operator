#!/usr/bin/env bash
set -euo pipefail

EXIT_CODE=0

# Determine the commit hash and tag of the previous release:
#
#     * If the repository head is at a release tag (e.g. v1.2.3-rc4), select
#       the release before that (e.g. v1.2.3-rc3)
#     * If the repository head has advanced from a release tag (e.g. v1.0-g0123abcd),
#       select the release the current head is based on (e.g. v1.0)
if $(git describe HEAD --tags | grep -Eq "^v[0-9]+(\.[0-9]+)*(-[a-z0-9]+)?$"); then
    LAST_RELEASE_HASH=$(git rev-list --tags --max-count=1 --skip=1 --no-walk)
else
    TAG=$(git describe HEAD --tags | grep -Eo "^v[0-9]+(\.[0-9]+)*")
    LAST_RELEASE_HASH=$(git rev-list -1 "${TAG}")
fi
LAST_RELEASE_TAG=$(git describe "${LAST_RELEASE_HASH}" --tags)

cd "${CHARTS_DIR}"
echo "📝 Checking for updated Chart versions since the last brupop release ${LAST_RELEASE_TAG}"
for d in */; do
    LAST_COMMIT_HASH=$(git --no-pager log --pretty=tformat:"%H" -- "${d}" | awk 'FNR <= 1')
    if [[ -z "${LAST_COMMIT_HASH}" ]]; then
        echo "✅ Chart ${d} has not been commited and appears to be in development"
        continue
    fi

    ## If LAST_RELEASE_HASH does not include the chart, then it's a new chart and does not need a version increment
    if [[ -z $(git ls-tree -d "${LAST_RELEASE_HASH}" "${d}") ]]; then
        echo "✅ Chart ${d} is a new chart since the last release"
        continue
    fi

    ## If LAST_RELEASE_HASH is NOT an ancestor of LAST_COMMIT_HASH then it has not been modified
    if [[ -z $(git rev-list "${LAST_COMMIT_HASH}" | grep "${LAST_RELEASE_HASH}") || "${LAST_COMMIT_HASH}" == "${LAST_RELEASE_HASH}" ]]; then
        echo "✅ Chart ${d} had no changes since the last eks-charts release"
        continue
    fi

    LAST_RELEASE_CHART_VERSION=$(git --no-pager show "${LAST_RELEASE_HASH}":deploy/charts/"${d}"Chart.yaml | awk '$1 == "version:" { print $2 }' | tr -d '[:space:]')
    LAST_COMMIT_CHART_VERSION=$(git --no-pager show "${LAST_COMMIT_HASH}":deploy/charts/"${d}"Chart.yaml | awk '$1 == "version:" { print $2 }' | tr -d '[:space:]')
    if [[ "${LAST_RELEASE_CHART_VERSION}" == "${LAST_COMMIT_CHART_VERSION}" ]]; then
        echo "❌ Chart ${d} has the same Chart version as the last release ${LAST_COMMIT_CHART_VERSION}"
        EXIT_CODE=1
    else
        echo "✅ Chart ${d} has a different version since the last eks-charts release (${LAST_RELEASE_CHART_VERSION} -> ${LAST_COMMIT_CHART_VERSION})"
    fi
done
exit $EXIT_CODE
