# Arch Linux / AUR

The release pipeline produces `OmaScribe_VERSION_Arch-PKGBUILD.tar.gz`, containing
`PKGBUILD`, `.SRCINFO`, and the application license. The pinned deb checksum is
computed from the tested installer, never `SKIP`. Bundled model licenses stay in
the native resources. This package uses pacman for upgrades; the app does not
replace a package-managed binary.

Download the recipe from the matching GitHub Release, inspect it, then:

```sh
tar xf OmaScribe_VERSION_Arch-PKGBUILD.tar.gz
cd omascribe-bin
makepkg -si
```

The release CI builds and installs this recipe in an Arch Linux container. It
checks dependency resolution and the installed CLI; graphical desktop behavior
still requires an Arch desktop test.

**AUR listing is pending an authorized AUR maintainer account.** Do not advertise
`yay -S omascribe-bin` as available until the repository is published. Once an
account is ready, register its SSH public key, clone
`ssh://aur@aur.archlinux.org/omascribe-bin.git`, copy the three recipe files,
review `makepkg --printsrcinfo`, commit and push. Never upload installers to AUR.

Reference: https://v2.tauri.app/distribute/aur/
