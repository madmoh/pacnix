pkgname=pacnix
pkgver=0.5.0
pkgrel=1
pkgdesc='Declarative, reconciling management of an Arch Linux host from a Nix flake'
arch=(x86_64)
license=(MIT)
depends=(aurutils base-devel curl devtools gcc-libs git glibc libarchive nix pacman sudo systemd util-linux)
makedepends=(cargo)
options=(!debug)

build() {
  cd "$startdir"
  cargo build --release --locked --target-dir "$srcdir/target"
}

package() {
  install -Dm755 "$srcdir/target/release/pacnix" "$pkgdir/usr/bin/pacnix"
  install -Dm644 "$startdir/LICENSE" "$pkgdir/usr/share/licenses/pacnix/LICENSE"
  cd "$startdir/nix"
  find . -type f -exec install -Dm644 {} "$pkgdir/usr/share/pacnix/{}" \;
  cd "$startdir/pkg"
  install -Dm644 sysusers.conf "$pkgdir/usr/lib/sysusers.d/pacnix.conf"
  install -Dm644 tmpfiles.conf "$pkgdir/usr/lib/tmpfiles.d/pacnix.conf"
}
