#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

node - "$repo_root" <<'NODE'
const fs = require("fs");
const path = require("path");

const repoRoot = process.argv[2];

function read(file) {
  return fs.readFileSync(path.join(repoRoot, file), "utf8");
}

function readJson(file) {
  return JSON.parse(read(file));
}

function cargoVersion(file) {
  const match = read(file).match(/^version\s*=\s*"([^"]+)"/m);
  if (!match) {
    throw new Error(`Could not find package version in ${file}`);
  }
  return match[1];
}

const versions = {
  "Cargo.toml": cargoVersion("Cargo.toml"),
  "desktop/package.json": readJson("desktop/package.json").version,
  "desktop/src-tauri/Cargo.toml": cargoVersion("desktop/src-tauri/Cargo.toml"),
  "desktop/src-tauri/tauri.conf.json": readJson("desktop/src-tauri/tauri.conf.json").version,
};

const unique = new Set(Object.values(versions));
if (unique.size !== 1) {
  console.error("Desktop and CLI versions differ:");
  for (const [file, version] of Object.entries(versions)) {
    console.error(`  ${file}: ${version}`);
  }
  process.exit(1);
}

console.log(`agentscan CLI and desktop versions match: ${[...unique][0]}`);
NODE
