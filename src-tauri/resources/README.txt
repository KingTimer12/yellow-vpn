This directory holds binary resources that are bundled beside the yellow-vpn
GUI executable by Tauri (see the "bundle.resources" array in
src-tauri/tauri.conf.json). Neither file is committed to git.

Required manual step before running/bundling:

  Download amd64 wintun.dll from https://www.wintun.net/ and place it here
  as wintun.dll before running/bundling. Not committed to git.

The other resource, yellow-vpn-helper.exe, is produced automatically by the
`prebuild:helper` npm script (built from the crates/vpn-helper workspace
member) and copied here before `tauri build`. It also does not need to be
placed here manually.
