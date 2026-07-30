#!/usr/bin/env bash
set -euo pipefail

# Used in CI, extracted here for readability.
#
# Rewrite the version of the base package and all platform optionalDependencies
# to the version extracted from the release tag.
#
# Usage: update-base-package.sh <version>

VERSION="${1:?Missing version}"

echo "Updating base package.json to version $VERSION..."

# Find the package.json relative to this script
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PACKAGE_JSON="$SCRIPT_DIR/../package.json"

if [[ ! -f "$PACKAGE_JSON" ]]; then
  echo "❌ Error: package.json not found at $PACKAGE_JSON"
  exit 1
fi

# Update version in base package.json
sed -i.bak "s/\"version\": \".*\"/\"version\": \"$VERSION\"/" "$PACKAGE_JSON"

# Update optionalDependencies versions
sed -i.bak "s/\"nuwax-codex-ts-darwin-arm64\": \".*\"/\"nuwax-codex-ts-darwin-arm64\": \"$VERSION\"/" "$PACKAGE_JSON"
sed -i.bak "s/\"nuwax-codex-ts-darwin-x64\": \".*\"/\"nuwax-codex-ts-darwin-x64\": \"$VERSION\"/" "$PACKAGE_JSON"
sed -i.bak "s/\"nuwax-codex-ts-linux-arm64\": \".*\"/\"nuwax-codex-ts-linux-arm64\": \"$VERSION\"/" "$PACKAGE_JSON"
sed -i.bak "s/\"nuwax-codex-ts-linux-arm64-musl\": \".*\"/\"nuwax-codex-ts-linux-arm64-musl\": \"$VERSION\"/" "$PACKAGE_JSON"
sed -i.bak "s/\"nuwax-codex-ts-linux-x64\": \".*\"/\"nuwax-codex-ts-linux-x64\": \"$VERSION\"/" "$PACKAGE_JSON"
sed -i.bak "s/\"nuwax-codex-ts-linux-x64-musl\": \".*\"/\"nuwax-codex-ts-linux-x64-musl\": \"$VERSION\"/" "$PACKAGE_JSON"
sed -i.bak "s/\"nuwax-codex-ts-win32-arm64\": \".*\"/\"nuwax-codex-ts-win32-arm64\": \"$VERSION\"/" "$PACKAGE_JSON"
sed -i.bak "s/\"nuwax-codex-ts-win32-x64\": \".*\"/\"nuwax-codex-ts-win32-x64\": \"$VERSION\"/" "$PACKAGE_JSON"

# Remove backup file
rm -f "$PACKAGE_JSON.bak"

echo "✅ Updated package.json:"
cat "$PACKAGE_JSON"
