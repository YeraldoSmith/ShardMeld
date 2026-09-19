#!/bin/zsh
set -euo pipefail

repo_dir="${0:A:h:h}"
app_dir="$repo_dir/dist/ShardMeld.app"
engine="$app_dir/Contents/Resources/shardmeld"
expected_version="2.2.0"
work_dir="$(mktemp -d)"
trap 'rm -rf "$work_dir"' EXIT

[[ -d "$app_dir" ]] || { print -u2 "Missing $app_dir"; exit 1; }
[[ -x "$engine" ]] || { print -u2 "Missing bundled engine"; exit 1; }

codesign --verify --deep --strict --verbose=2 "$app_dir"
plist_version="$(plutil -extract CFBundleShortVersionString raw "$app_dir/Contents/Info.plist")"
[[ "$plist_version" == "$expected_version" ]] || {
  print -u2 "App version mismatch: $plist_version"
  exit 1
}

engine_version="$($engine --version)"
[[ "$engine_version" == "shardmeld $expected_version" ]] || {
  print -u2 "Engine version mismatch: $engine_version"
  exit 1
}

"$engine" capabilities --json "$work_dir/capabilities.json" >/dev/null
rg -q '"native-macos-analysis-and-receive-ui"' "$work_dir/capabilities.json"
rg -q 'Copyright \(C\) 2026 YeraldoSmith' "$app_dir/Contents/Resources/COPYRIGHT.txt"
rg -q 'GNU AFFERO GENERAL PUBLIC LICENSE' "$app_dir/Contents/Resources/LICENSE.txt"

print "verified_app=$app_dir"
print "app_version=$plist_version"
print "engine_version=$engine_version"
