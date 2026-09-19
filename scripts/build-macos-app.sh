#!/bin/zsh
set -euo pipefail

repo_dir="${0:A:h:h}"
source_dir="$repo_dir/apps/macos"
app_dir="$repo_dir/dist/ShardMeld.app"
contents_dir="$app_dir/Contents"
macos_dir="$contents_dir/MacOS"
resources_dir="$contents_dir/Resources"
sdk_path="$(xcrun --show-sdk-path)"
iconset_dir="$repo_dir/dist/ShardMeld.iconset"

if [[ "$(uname -m)" != "arm64" ]]; then
  print -u2 "This packaging script currently targets Apple Silicon."
  exit 1
fi

if [[ ! -x "$repo_dir/dist/shardmeld-macos-arm64" ]]; then
  print -u2 "Missing signed engine: dist/shardmeld-macos-arm64"
  exit 1
fi

rm -rf "$app_dir"
mkdir -p "$macos_dir" "$resources_dir"

swiftc \
  -parse-as-library \
  -target arm64-apple-macos13.0 \
  -sdk "$sdk_path" \
  -framework SwiftUI \
  -framework AppKit \
  -o "$macos_dir/ShardMeld" \
  "$source_dir/ShardMeldApp.swift" \
  "$source_dir/AppModel.swift" \
  "$source_dir/EngineRunner.swift" \
  "$source_dir/Views.swift"

cp "$source_dir/Info.plist" "$contents_dir/Info.plist"
cp "$repo_dir/dist/shardmeld-macos-arm64" "$resources_dir/shardmeld"
cp "$repo_dir/LICENSE" "$resources_dir/LICENSE.txt"
cp "$repo_dir/COPYRIGHT" "$resources_dir/COPYRIGHT.txt"
rm -rf "$iconset_dir"
swift "$repo_dir/scripts/make-macos-icon.swift" "$iconset_dir"
iconutil --convert icns --output "$resources_dir/ShardMeld.icns" "$iconset_dir"
rm -rf "$iconset_dir"
chmod 755 "$macos_dir/ShardMeld" "$resources_dir/shardmeld"

codesign --force --deep --sign - "$app_dir"
codesign --verify --deep --strict --verbose=2 "$app_dir"

print "$app_dir"
