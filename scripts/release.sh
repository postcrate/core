#!/usr/bin/env bash
#
# Open a release PR for postcrate-core.
#
#   ./scripts/release.sh 0.2.0
#
# What it does:
#   1. Verifies you are on the `dev` branch with a clean working tree.
#   2. Pulls latest `dev`.
#   3. Bumps the workspace version across Cargo.toml (workspace.package and
#      workspace.dependencies path-dep stay in sync).
#   4. Runs `cargo check --workspace` to confirm the bump compiles.
#   5. Commits as "release: vX.Y.Z" and pushes `dev`.
#   6. Opens a PR `dev` -> `main`.
#
# After merging the PR, the Release workflow on `main` tags, publishes to
# crates.io, and creates the GitHub release automatically.
#
# Requires: cargo-edit (`cargo install cargo-edit`), gh CLI authenticated.

set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "Usage: $0 <version>" >&2
  echo "Example: $0 0.2.0" >&2
  exit 1
fi

VERSION="$1"

if [[ ! "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?$ ]]; then
  echo "Error: '$VERSION' is not valid semver (e.g. 0.2.0, 1.0.0-beta.1)" >&2
  exit 1
fi

if ! cargo set-version --help >/dev/null 2>&1; then
  echo "Error: 'cargo set-version' not found. Install with:" >&2
  echo "  cargo install cargo-edit" >&2
  exit 1
fi

if ! command -v gh >/dev/null 2>&1; then
  echo "Error: 'gh' CLI not found. Install from https://cli.github.com/" >&2
  exit 1
fi

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

CURRENT_BRANCH="$(git rev-parse --abbrev-ref HEAD)"
if [[ "$CURRENT_BRANCH" != "dev" ]]; then
  echo "Error: must be on the 'dev' branch (currently on '$CURRENT_BRANCH')." >&2
  echo "Switch with: git checkout dev" >&2
  exit 1
fi

if ! git diff --quiet || ! git diff --cached --quiet; then
  echo "Error: working tree has uncommitted changes. Commit or stash first." >&2
  exit 1
fi

echo "==> Pulling latest dev"
git pull --ff-only

echo "==> Bumping workspace version to $VERSION"
cargo set-version --workspace "$VERSION"

echo "==> Verifying the bump compiles"
cargo check --workspace --quiet

echo "==> Committing and pushing"
git add Cargo.toml crates/*/Cargo.toml
git commit -m "release: v$VERSION"
git push origin dev

echo "==> Opening release PR"
PR_URL=$(gh pr create \
  --base main \
  --head dev \
  --title "release: v$VERSION" \
  --body "Bumps \`postcrate-core\` to \`v$VERSION\`.

After merge, the Release workflow on \`main\` will:
1. Create and push tag \`v$VERSION\`
2. Publish \`postcrate-core $VERSION\` to crates.io
3. Create the GitHub release")

echo ""
echo "Release PR opened: $PR_URL"
echo "Review and merge. The Release workflow on main will do the rest."
