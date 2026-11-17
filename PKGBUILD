pkgbase=pacnix
pkgname=(pacnix pacnix-keyring)
pkgver=0.7.0
pkgrel=1
pkgdesc='Declarative, reconciling management of an Arch Linux host from a Nix flake'
arch=(x86_64)
license=(MIT)
makedepends=(cargo)
options=(!debug)

build() {
  cd "$startdir"
  cargo build --release --locked --target-dir "$srcdir/target"
}

package_pacnix() {
  depends=(aurutils base-devel curl devtools gcc-libs git glibc libarchive nix pacman pacnix-keyring sudo systemd util-linux)
  install -Dm755 "$srcdir/target/release/pacnix" "$pkgdir/usr/bin/pacnix"
  install -Dm644 "$startdir/LICENSE" "$pkgdir/usr/share/licenses/pacnix/LICENSE"
  cd "$startdir/nix"
  find . -type f -exec install -Dm644 {} "$pkgdir/usr/share/pacnix/{}" \;
  cd "$startdir/pkg"
  install -Dm644 sysusers.conf "$pkgdir/usr/lib/sysusers.d/pacnix.conf"
  install -Dm644 tmpfiles.conf "$pkgdir/usr/lib/tmpfiles.d/pacnix.conf"
}

# The key of the [pacnix] repository for pacman-key --populate, as with
# archlinux-keyring. PACNIX_KEY selects a different key, for the tests.
package_pacnix-keyring() {
  pkgdesc='The key that signs the [pacnix] repository'
  arch=(any)
  install=pkg/pacnix-keyring.install
  local key=${PACNIX_KEY:-$startdir/pacnix.asc}
  mkdir -pm700 "$srcdir/gnupg"
  local fpr
  fpr=$(gpg --homedir "$srcdir/gnupg" --with-colons --show-keys "$key" | awk -F: '/^fpr/ { print $10; exit }')
  install -Dm644 "$key" "$pkgdir/usr/share/pacman/keyrings/pacnix.gpg"
  echo "$fpr:4:" | install -Dm644 /dev/stdin "$pkgdir/usr/share/pacman/keyrings/pacnix-trusted"
  install -Dm644 "$startdir/LICENSE" "$pkgdir/usr/share/licenses/pacnix-keyring/LICENSE"
}
