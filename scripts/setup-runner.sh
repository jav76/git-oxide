#!/usr/bin/env bash
set -euo pipefail

REPO="jav76/git-oxide"
RUNNER_DIR="${1:-$HOME/actions-runner-git-oxide}"
RUNNER_NAME="${RUNNER_NAME:-$(hostname)-git-oxide}"
RUNNER_LABELS="self-hosted,Linux,X64,git-oxide"

if ! command -v gh >/dev/null 2>&1; then
    echo "Error: GitHub CLI (gh) is required." >&2
    exit 1
fi

echo "Configuring repository variable RUNNER_LABELS on $REPO..."
gh variable set RUNNER_LABELS --repo "$REPO" --body '["self-hosted","Linux","X64","git-oxide"]'

echo "Requesting registration token from GitHub for $REPO..."
token="$(gh api -X POST "repos/$REPO/actions/runners/registration-token" --jq '.token')"

mkdir -p "$RUNNER_DIR"
cd "$RUNNER_DIR"

if [ ! -f ./config.sh ]; then
    latest_ver="$(gh api repos/actions/runner/releases/latest --jq '.tag_name' | sed 's/^v//')"
    archive="actions-runner-linux-x64-${latest_ver}.tar.gz"
    url="https://github.com/actions/runner/releases/download/v${latest_ver}/${archive}"
    echo "Downloading actions-runner v${latest_ver} into $RUNNER_DIR..."
    curl -fL "$url" -o "$archive"
    tar xzf "$archive"
    rm -f "$archive"
fi

./config.sh \
    --url "https://github.com/$REPO" \
    --token "$token" \
    --name "$RUNNER_NAME" \
    --labels "$RUNNER_LABELS" \
    --unattended \
    --replace

echo
echo "Runner configured in $RUNNER_DIR with labels: $RUNNER_LABELS"
echo "Start interactively with: cd $RUNNER_DIR && ./run.sh"
echo "Or install as a systemd service: cd $RUNNER_DIR && sudo ./svc.sh install && sudo ./svc.sh start"
