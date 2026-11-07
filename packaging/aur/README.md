# AUR packaging

This directory contains a **proposed** `lyricflow-bin` package for Arch Linux
x86_64. It downloads the published 0.1.1 binary and verifies its SHA256 checksum.
The package installs to `/usr/bin/lyricflow` and does not need Rust at runtime.

This template has not been submitted to AUR. Package-name availability and a
complete Arch clean-chroot build have not been verified in the development
environment. Before publishing, check that the name is free and review the
[AUR submission guidelines](https://wiki.archlinux.org/title/AUR_submission_guidelines).

## Build and validate on Arch

Install the packaging tools once:

```sh
sudo pacman -S --needed base-devel git namcap
```

From this repository:

```sh
cd packaging/aur/lyricflow-bin
makepkg --verifysource
makepkg --cleanbuild --syncdeps
namcap PKGBUILD
namcap lyricflow-bin-*.pkg.tar.zst
makepkg --printsrcinfo > .SRCINFO
```

Investigate packaging warnings before submission. A clean build with Arch's
`devtools` is recommended. To install the locally built package:

```sh
sudo pacman -U lyricflow-bin-*.pkg.tar.zst
/usr/bin/lyricflow --version
/usr/bin/lyricflow demo
```

A previous Cargo installation in `~/.cargo/bin` may take precedence on PATH.
Use the explicit `/usr/bin/lyricflow` path while validating the package. Once
the package works, `cargo uninstall lyricflow` removes the Cargo copy without
deleting Spotify credentials or settings.

## Submit to AUR

Create an account at https://aur.archlinux.org/ and add an SSH **public** key to
that account. Publishing requires your AUR account and its SSH access; a GitHub
repository does not create an AUR package automatically. Do not share private
keys or account credentials.

Clone the new AUR package repository into a separate directory, then copy only
`PKGBUILD` and the freshly generated `.SRCINFO` into it:

```sh
git clone ssh://aur@aur.archlinux.org/lyricflow-bin.git
```

If that name already exists, do not overwrite another maintainer's package.
Choose an appropriate available name and update both metadata files.

Inside your AUR checkout:

```sh
git add PKGBUILD .SRCINFO
git commit -m 'Initial package: lyricflow-bin 0.1.1'
git push origin HEAD:master
```

Do not upload package archives, binaries, build directories, local settings or
tokens. Git commit author information is public: choose your intended public
name and email, such as your GitHub noreply address, before committing.

## Updating

When a new upstream version is released, update `pkgver` and its actual release
archive checksum, reset `pkgrel` to 1, rebuild, rerun `namcap`, regenerate
`.SRCINFO`, commit and push. Never use `SKIP` in place of the release checksum.
For packaging-only changes, increment `pkgrel` instead of `pkgver`.
