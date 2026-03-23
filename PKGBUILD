# Maintainer: Jeremiah Wenzel <jwenzel2@github.com>
pkgname=wrustyssh
pkgver=1.9.0
pkgrel=1
pkgdesc='A GTK4/libadwaita SSH client with tabbed terminals'
arch=('x86_64')
url='https://github.com/jwenzel2/wrustyssh'
license=('MIT')
depends=('gtk4' 'libadwaita' 'vte4')
makedepends=('rust' 'cargo' 'pkgconf')
source=()

build() {
    cd "$startdir"
    cargo build --release --locked
}

package() {
    cd "$startdir"
    install -Dm755 "target/release/$pkgname" "$pkgdir/usr/bin/$pkgname"
    install -Dm644 "data/$pkgname.desktop" "$pkgdir/usr/share/applications/$pkgname.desktop"
    install -Dm644 "icon.png" "$pkgdir/usr/share/icons/hicolor/256x256/apps/$pkgname.png"
}
