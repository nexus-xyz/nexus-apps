# Linux packages

This folder makes Nexus Desktop's Linux packages. It is for whoever releases the desktop app or changes how it installs on Linux.

`package.sh` builds the release tarball, `nexus-desktop-<ver>-linux-x86_64.tar.xz`. `packages.sh` turns that tarball into three more packages:

| File | For | Installs to | Updates |
| --- | --- | --- | --- |
| `nexus-desktop_<ver>_amd64.deb` | Debian 12, Ubuntu 22.04 and later | `/opt/nexus-desktop` | re-install from the new release file |
| `nexus-desktop-<ver>-1.x86_64.rpm` | Fedora, RHEL 10, openSUSE | `/opt/nexus-desktop` | re-install from the new release file |
| `nexus-desktop-<ver>-linux-x86_64.AppImage` | any x86_64 distribution with FUSE 3 | nowhere: it runs in place | a new download |
| `aur/nexus-desktop-bin/PKGBUILD` | Arch Linux, from the tarball | `/opt/nexus-desktop` | pacman, an AUR helper |

- Every system package puts `nexus-desktop-gpui` on `PATH`, adds the menu entry and opens `nexus://` links.
- None of them ships `install.sh`. Without it beside the binary, the app's own updater stays off (`src/updates/install.rs`, `packaged`), so it never tries to overwrite files owned by the package. The tarball keeps `install.sh` and keeps updating itself.
- The binary needs glibc 2.35 or later. `nfpm.yaml` lists the libraries the binary and CEF link, as Debian packages and as RPM sonames. The PKGBUILD lists the same libraries as Arch packages.

## Release

A release runs `packages.sh` on Linux, after `package.sh`. It needs nfpm, appimagetool and the AppImage runtime, installed at pinned versions with pinned sha256 sums. The feed signs each package, lists it in `SHA256SUMS` and in `latest.json` under `downloads` (`linux-x86_64-deb`, `linux-x86_64-rpm`, `linux-x86_64-appimage`), and never offers it as an update.

Publish the AUR package after legal sign-off and the first stable release that carries the approved license.
Run the renderer from that release's Git tag so the license in the recipe matches the tarball.

```sh
curl -fsS -o /tmp/latest.json https://downloads.nexus.inc/desktop/stable/latest.json
packaging/linux/aur/render.sh /tmp/latest.json
(
  cd packaging/linux/aur/nexus-desktop-bin
  makepkg --printsrcinfo | diff -u .SRCINFO - && makepkg -si
)
```

Copy `PKGBUILD` and `.SRCINFO` to a separate checkout of the AUR package repository.
Commit both files there. Push the AUR checkout to `master` after release approval.

```sh
cd /path/to/aur/nexus-desktop-bin
git push origin HEAD:master
```

## Build one locally

```sh
packaging/package.sh                                   # or download a release tarball
packaging/linux/packages.sh target/package/nexus-desktop-*-linux-x86_64.tar.xz target/package
```

It needs `nfpm` and `appimagetool` on `PATH`. Set `APPIMAGE_RUNTIME` to a downloaded runtime file to avoid appimagetool's own download.

## Not yet

- An apt and a dnf repository, so `apt upgrade` finds new versions. Both need a signing key.
- arm64 packages. The release builds Linux only for x86_64.
- Snap and Flatpak.
