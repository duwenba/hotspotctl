# Maintainer: Du Wenba <2275317692@qq.com>
# 本地源码打包: 在仓库根目录执行 `makepkg -f` 生成软件包, `pacman -U` 安装。
pkgname=hotspotctl
pkgver=0.1.0
pkgrel=1
pkgdesc='Linux 便携热点管理器 (NetworkManager + UFW, Rust TUI/CLI)'
arch=(x86_64)
url='https://local/hotspot-manager'
license=(custom)
depends=(networkmanager ufw dnsmasq iw)
makedepends=(cargo)
install=
changelog=
source=()
noextract=()
validpgpkeys=()

build() {
  # 无 source, 直接在当前源码树构建(复用已有 target 缓存)。
  cd "$startdir"
  export CARGO_TERM_COLOR=never
  cargo build --release --frozen
}

check() {
  cd "$startdir"
  cargo test --release --frozen
}

package() {
  cd "$startdir"
  install -Dm755 target/release/hotspotctl -t "$pkgdir/usr/bin/"

  # 打包版服务文件: 使用 /usr/bin/hotspotctl, 去掉开发机硬编码路径。
  mkdir -p "$pkgdir/usr/lib/systemd/system"
  sed -e '/^WorkingDirectory=/d' \
      -e 's|^ExecStart=.*|ExecStart=/usr/bin/hotspotctl on|' \
      -e 's|^ExecStop=.*|ExecStop=/usr/bin/hotspotctl off|' \
      -e 's|^Documentation=.*|Documentation=file:///usr/share/doc/hotspotctl/README.md|' \
      hotspotctl.service > "$pkgdir/usr/lib/systemd/system/hotspotctl.service"

  install -Dm644 README.md -t "$pkgdir/usr/share/doc/hotspotctl/"
}
