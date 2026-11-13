# Maintainer: Du Wenba <2275317692@qq.com>
# 本地源码打包: 在仓库根目录执行 `makepkg -f` 生成软件包, `pacman -U` 安装。
pkgname=hotspotctl
pkgver=0.1.2
pkgrel=1
pkgdesc='Linux 便携热点管理器 (NetworkManager + UFW, Rust TUI/CLI)'
arch=(x86_64)
url='https://github.com/duwenba/hotspotctl'
license=(custom)
depends=(networkmanager ufw dnsmasq iw)
makedepends=(cargo)
install=
changelog=
source=()
noextract=()
validpgpkeys=()
# 已在 Cargo.toml 里 strip, 不需要单独的 debug 包。
options=(!debug)

build() {
  # 无 source, 直接在当前源码树构建(复用已有 target 缓存)。
  cd "$startdir"
  export CARGO_TERM_COLOR=never
  cargo build --release --locked
}

check() {
  cd "$startdir"
  cargo test --release --locked
}

package() {
  cd "$startdir"
  install -Dm755 target/release/hotspotctl -t "$pkgdir/usr/bin/"

  # 打包版服务文件: 使用 /usr/bin/hotspotctl, 无开发机硬编码路径。
  install -Dm644 packaging/hotspotctl.service \
    "$pkgdir/usr/lib/systemd/system/hotspotctl.service"

  install -Dm644 README.md -t "$pkgdir/usr/share/doc/hotspotctl/"
}
