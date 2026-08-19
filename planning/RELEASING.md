# Releasing

## Building locally

```sh
npm run build
```

Artifacts land in `target/release/bundle/`: `dmg/` and `macos/` on macOS,
`nsis/` and `msi/` on Windows. A local build is unsigned, so macOS quarantines
it — the first launch needs a right click and "Open", or

```sh
xattr -dr com.apple.quarantine "target/release/bundle/macos/Typst Studio.app"
```

Cross-compiling between macOS and Windows is not practical; CI builds both.

## Releasing through CI

`.github/workflows/release.yml` runs on a `v*` tag and produces a draft release
with installers for Apple Silicon, Intel macOS, and Windows. Publish the draft
once the artifacts have been checked.

```sh
npm version 0.2.0 --no-git-tag-version   # keeps package.json in step
# update version in Cargo.toml and src-tauri/tauri.conf.json to match
git commit -am "Release 0.2.0"
git tag v0.2.0 && git push --tags
```

The three version numbers (workspace `Cargo.toml`, `tauri.conf.json`, root
`package.json`) must agree; the updater compares against the one in
`tauri.conf.json`.

## macOS signing and notarization

Without a Developer ID, users get "Apple could not verify this app". Signing
needs an Apple Developer account (99 USD a year) and these repository secrets:

| Secret | What it is |
|---|---|
| `APPLE_CERTIFICATE` | Developer ID Application certificate, `.p12`, base64 encoded |
| `APPLE_CERTIFICATE_PASSWORD` | Password used when exporting the `.p12` |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Name (TEAMID)` |
| `APPLE_ID` | Apple ID used for notarization |
| `APPLE_PASSWORD` | App-specific password for that Apple ID |
| `APPLE_TEAM_ID` | Ten-character team identifier |

Export the certificate with

```sh
security find-identity -v -p codesigning        # find the identity name
base64 -i certificate.p12 | pbcopy              # value for APPLE_CERTIFICATE
```

The workflow passes these to `tauri-action`, which signs and notarizes. With the
secrets absent the build still succeeds and produces unsigned artifacts.

Verify a signed build:

```sh
codesign --verify --deep --strict --verbose=2 "Typst Studio.app"
spctl --assess --type execute --verbose "Typst Studio.app"
```

## Windows signing

Unsigned installers trigger SmartScreen. Signing needs a code signing
certificate from a certificate authority; an EV certificate builds SmartScreen
reputation immediately, a standard one accumulates it over time. Configure it
under `bundle.windows.certificateThumbprint` in `tauri.conf.json`, or sign the
artifacts in CI after the build.

## Automatic updates

Not enabled yet. It needs a signing key pair and somewhere to publish the update
manifest, and both are deployment decisions rather than code:

1. Generate a key pair — keep the private key out of the repository:

   ```sh
   npx tauri signer generate -w ~/.typst-studio/updater.key
   ```

2. Add `tauri-plugin-updater` to `src-tauri`, register it in `lib.rs`, and add
   `updater:default` to `src-tauri/capabilities/default.json`.

3. In `tauri.conf.json`, set `plugins.updater.pubkey` to the public key and
   `plugins.updater.endpoints` to the manifest URL, and add
   `"createUpdaterArtifacts": true` to the bundle.

4. Store the private key as `TAURI_SIGNING_PRIVATE_KEY` (and its password) in
   repository secrets — the release workflow already passes them through.

5. Have the app check for updates on startup and offer, not force, the install.

Publishing the manifest to GitHub Releases works, but pins updates to a
repository that must stay public.

## Notes on the disk image

The image deliberately carries no license agreement. Setting `licenseFile` in
the bundle makes macOS demand agreement before the image will mount, which adds
a dialog to every install and blocks scripted mounts. The license ships in the
repository and inside the app instead.

## Checklist for a release

- `cargo test --workspace` and `npm --prefix ui run check` pass.
- Versions agree across `Cargo.toml`, `src-tauri/tauri.conf.json`, and
  `package.json`.
- `hdiutil verify` on the image passes, and it mounts without a dialog.
- The app copied out of the image starts with a fresh user profile.
- Documents in both languages compile and export:

  ```sh
  cargo run --release --example checklist -p typst-studio-core -- <project> main.typ
  pdffonts out.pdf    # every font should show emb=yes
  pdftotext out.pdf - # text, including Cyrillic, should come back
  ```

- By hand, in the installed app: open a folder, edit a file, export a PDF.
