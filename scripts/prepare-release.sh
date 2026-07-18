#!/usr/bin/env bash
set -euo pipefail

usage() {
  echo "Usage: scripts/prepare-release.sh <version>" >&2
  echo "  <version> must be a bare semantic version such as 0.11.0" >&2
}

if [[ $# -ne 1 || ! $1 =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  usage
  exit 1
fi

version="$1"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

current_version="$(
  node - "$repo_root/Cargo.toml" <<'NODE'
const fs = require("fs");

const contents = fs.readFileSync(process.argv[2], "utf8");
const packageSection = contents.match(/^\[package\][^\r\n]*(?:\r?\n)[\s\S]*?(?=^\[|(?![\s\S]))/m);
const version = packageSection && packageSection[0].match(/^version\s*=\s*"([^"]+)"/m);

if (!version) {
  console.error("Could not find the package version in Cargo.toml");
  process.exit(1);
}

process.stdout.write(version[1]);
NODE
)"

if [[ "$version" == "$current_version" || "$(printf '%s\n%s\n' "$current_version" "$version" | sort -V | tail -n 1)" != "$version" ]]; then
  echo "Error: release version $version must be strictly greater than current version $current_version" >&2
  exit 1
fi

node - "$repo_root/CHANGELOG.md" <<'NODE'
const fs = require("fs");

const contents = fs.readFileSync(process.argv[2], "utf8");
const match = contents.match(/^## Unreleased[^\r\n]*(?:\r?\n)([\s\S]*?)(?=^## |(?![\s\S]))/m);

if (!match) {
  console.error("Error: CHANGELOG.md does not contain a valid ## Unreleased section");
  process.exit(1);
}

if (!match[1].split(/\r?\n/).some((line) => line.trim() !== "")) {
  console.error("Error: the ## Unreleased section of CHANGELOG.md is empty");
  process.exit(1);
}
NODE

if [[ -n "$(git status --porcelain)" ]]; then
  echo "Error: git working tree is dirty; commit or stash changes before preparing a release" >&2
  exit 1
fi

today="$(date +%Y-%m-%d)"

node - "$repo_root" "$version" "$today" <<'NODE'
const fs = require("fs");
const path = require("path");

const repoRoot = process.argv[2];
const version = process.argv[3];
const today = process.argv[4];

function updateCargoVersion(relativePath) {
  const file = path.join(repoRoot, relativePath);
  const contents = fs.readFileSync(file, "utf8");
  const updated = contents.replace(
    /(^\[package\][^\r\n]*(?:\r?\n)[\s\S]*?)(?=^\[|(?![\s\S]))/m,
    (section) => section.replace(
      /(^version\s*=\s*")[^"]+(".*$)/m,
      `$1${version}$2`,
    ),
  );
  if (updated === contents) {
    throw new Error(`Could not update [package] version in ${relativePath}`);
  }
  fs.writeFileSync(file, updated);
}

function updateJsonVersion(relativePath) {
  const file = path.join(repoRoot, relativePath);
  const contents = fs.readFileSync(file, "utf8");
  const updated = contents.replace(
    /^(  "version": ")[^"]+("[,]?)$/m,
    `$1${version}$2`,
  );
  if (updated === contents) {
    throw new Error(`Could not update version in ${relativePath}`);
  }
  fs.writeFileSync(file, updated);
}

updateCargoVersion("Cargo.toml");
updateJsonVersion("desktop/package.json");
updateCargoVersion("desktop/src-tauri/Cargo.toml");
updateJsonVersion("desktop/src-tauri/tauri.conf.json");

const changelogFile = path.join(repoRoot, "CHANGELOG.md");
const changelog = fs.readFileSync(changelogFile, "utf8");
const updatedChangelog = changelog.replace(
  /^## Unreleased$/m,
  `## Unreleased\n\n## ${version} - ${today}`,
);
if (updatedChangelog === changelog) {
  throw new Error("Could not roll the ## Unreleased section in CHANGELOG.md");
}
fs.writeFileSync(changelogFile, updatedChangelog);
NODE

cargo update -w
(
  cd desktop/src-tauri
  cargo update -w
)

scripts/check-desktop-version.sh

echo
echo "Prepared agentscan $version for release ($today):"
echo "  - bumped CLI and desktop versions, including both Cargo lockfiles"
echo "  - rolled CHANGELOG.md and opened a fresh Unreleased section"
echo "Review the diff, then commit and tag the release. This script did not commit, tag, or push."
