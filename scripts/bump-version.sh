#!/usr/bin/env bash
set -euo pipefail

if [ "$#" -ne 1 ]; then
    echo "Usage: $0 <semver-version> (e.g. 0.2.0 or 0.2.0-rc.1)" >&2
    exit 1
fi

new_version="${1#v}"
semver_regex="^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?(\+[0-9A-Za-z.-]+)?$"
if [[ ! "$new_version" =~ $semver_regex ]]; then
    echo "Error: '$1' is not a valid SemVer 2.0 version." >&2
    exit 1
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

current_version="$(grep -m1 '^version = ' Cargo.toml | cut -d'"' -f2)"
echo "Bumping workspace version: $current_version -> $new_version"

# Update the first `version = "..."` line under [workspace.package]
awk -v ver="$new_version" '
    !done && /^version = "/ {
        print "version = \"" ver "\""
        done = 1
        next
    }
    { print }
' Cargo.toml > Cargo.toml.tmp
mv Cargo.toml.tmp Cargo.toml

today="$(date -u +%Y-%m-%d)"
if ! grep -q "^## \[$new_version\]" CHANGELOG.md; then
    awk -v ver="$new_version" -v day="$today" '
        /^## \[Unreleased\]/ {
            print
            print ""
            print "## [" ver "] - " day
            print ""
            print "### Added"
            print ""
            print "- TODO: describe changes in v" ver "."
            inserted = 1
            next
        }
        { print }
        END {
            if (!inserted) {
                print ""
                print "## [" ver "] - " day
            }
        }
    ' CHANGELOG.md > CHANGELOG.md.tmp
    mv CHANGELOG.md.tmp CHANGELOG.md
    echo "Added '## [$new_version] - $today' section to CHANGELOG.md (please fill in release notes)."
else
    echo "CHANGELOG.md already contains a '## [$new_version]' section."
fi

echo "Refreshing Cargo.lock..."
cargo metadata --format-version 1 >/dev/null

echo
echo "Next steps:"
echo "  1. Fill in release notes under '## [$new_version]' in CHANGELOG.md"
echo "  2. Commit changes: git add Cargo.toml Cargo.lock CHANGELOG.md && git commit -m \"chore(release): v$new_version\""
echo "  3. Tag & push:     git tag v$new_version && git push origin main v$new_version"
