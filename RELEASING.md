# Releasing Yellow VPN

Releases are **tag-driven**. Pushing a `vX.Y.Z` tag triggers
[`.github/workflows/release.yml`](.github/workflows/release.yml), which builds
every platform and publishes them to a GitHub Release for that tag.

## TL;DR

```bash
bun scripts/bump-version.mjs patch   # or minor | major | 1.4.0
# review the diff, then run the commands the script prints:
git commit -am "chore(release): vX.Y.Z"
git tag vX.Y.Z
git push origin HEAD --follow-tags
```

That's it — CI does the rest. Watch it under the repo's **Actions** tab; assets
land on the **Releases** page when it finishes.

## What the bump script does

`scripts/bump-version.mjs <patch|minor|major|x.y.z>` writes the new version into
the four files that carry it, then prints the exact tag commands:

| File | Why |
| --- | --- |
| `package.json` | frontend / tooling version |
| `src-tauri/tauri.conf.json` | **source of truth**; CI validates the tag against it, and it drives the Android `versionCode` |
| `src-tauri/Cargo.toml` | GUI crate |
| `Cargo.toml` (`[workspace.package]`) | inherited by `vpn-engine` / `vpn-ipc` / `vpn-helper` and the Android `.so` |

The Android `versionCode` is derived from the version as
`major*1_000_000 + minor*1_000 + patch`. Android refuses an upgrade whose
versionCode does not strictly increase, so the script **rejects** any bump that
would not grow it.

## What CI builds

| Job | Runner | Output |
| --- | --- | --- |
| `desktop` (Windows) | `windows-latest` | NSIS `-setup.exe` + `.msi` |
| `desktop` (macOS) | `macos-latest` | `.dmg` / `.app` (Apple Silicon, `aarch64`) |
| `desktop` (Linux) | `ubuntu-latest` | `.deb` / `.AppImage` |
| `android` | `ubuntu-latest` | signed arm64 `.apk` |

The `validate` job fails the whole run if the pushed tag doesn't match
`tauri.conf.json` — so bump **before** tagging.

macOS bundles are ad-hoc signed (`signingIdentity: "-"`); Windows/Linux are
unsigned. No Tauri auto-updater is configured.

## Required GitHub secrets

Set these under **Settings → Secrets and variables → Actions**:

| Secret | Purpose |
| --- | --- |
| `ANDROID_KEYSTORE_B64` | release keystore, base64-encoded (`base64 -w0 release.jks`) |
| `ANDROID_KEY_ALIAS` | key alias inside the keystore |
| `ANDROID_KEYSTORE_PASSWORD` | keystore + key password (same value) |

`GITHUB_TOKEN` is provided automatically.

### Generating an Android keystore (one time)

```bash
keytool -genkey -v -keystore release.jks -keyalg RSA -keysize 2048 \
  -validity 10000 -alias yellow-vpn
base64 -w0 release.jks   # paste output into ANDROID_KEYSTORE_B64
```

Keep `release.jks` and its passwords safe — **losing them means you can never
ship an update** to already-installed Android builds (the signature must match).
The keystore and `keystore.properties` are gitignored; CI materializes them from
secrets and deletes them after the build.

## Local release build (optional smoke test)

```bash
bun run tauri:build          # desktop bundle for the current OS
bun run android:build        # unsigned release APK (no keystore.properties)
```
