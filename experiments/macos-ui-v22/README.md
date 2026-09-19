# ShardMeld 2.2 native macOS UI acceptance

Date: 2026-09-19

Platform: Apple Silicon macOS 15.5

Artifact: `dist/ShardMeld.app`

## What was exercised

The exact ad-hoc-signed application package was launched, not a source preview.
Through the visible native UI, the operator selected:

- authorized material: `experiments/smoke-m/sources/`;
- target: `experiments/smoke-m/target-v2.bin`.

The app invoked its bundled `Contents/Resources/shardmeld` engine, created a
fresh private run directory under Application Support, indexed the authorized
folder, described the target, compared both, and returned to the overview.

Observed result:

- target bytes: 16,842,753;
- locally reusable bytes: 16,361,172;
- missing payload bytes: 481,581;
- matched chunks: 192 of 198;
- displayed reuse ratio: 97.1%.

The same visible packaged-app flow was repeated after the final monochrome UI
redesign and app-icon packaging. `scripts/check-macos-app.sh` separately checks
the deep ad-hoc signature, app/engine version agreement, bundled copyright and
AGPL license, and the declared native UI capability.

## Evidence limits

This run validates the packaged local analysis path and its UI presentation.
It does not repeat qBittorrent interoperability, prove public-swarm behavior,
exercise every receive error path, provide Apple notarization, or claim DHT,
PEX, multi-file, v2/hybrid, background scanning, or an SMD economy UI.
