#!/usr/bin/env bash
set -euo pipefail

VERSION="$(git describe --tags --always)"

if [[ "${VERSION}" =~ ^v[0-9]+(\.[0-9]+){2}$ ]]; then
    git fetch --all
    git config user.name "${GITHUB_ACTOR}"
    git config user.email "${GITHUB_ACTOR}@users.noreply.github.com"
    git remote set-url origin "https://x-access-token:${GITHUB_TOKEN}@github.com/${GITHUB_REPO}"
    git config pull.rebase false
    git checkout gh-pages
    git checkout -b "gh-pages-release-${VERSION}"
    mv --update=none "${CHART_BUILD_DIR}"/charts/*.tgz .
    helmv3 repo index . --url "https://${GITHUB_REPO_OWNER}.github.io/${GITHUB_REPO_NAME}"
    git add ./*.tgz index.yaml
    git commit -m "Publish helm charts for ${VERSION}"

    git push origin "gh-pages-release-${VERSION}"
    gh pr create -B gh-pages -H "gh-pages-release-${VERSION}" \
        --title "Publish helm charts for ${VERSION}" \
        --body "This publishes charts for the Bottlerocket Update Operator version ${VERSION} to the helm repository."
else
    echo "Not a valid semver release tag! Skip charts publish"
    exit 1
fi
