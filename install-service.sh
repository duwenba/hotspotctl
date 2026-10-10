#!/usr/bin/env bash
# 安装 hotspotctl 为 systemd 服务。
# 会把仓库内的 hotspotctl.service 拷到 /etc/systemd/system/,并把其中的
# 仓库路径 / 二进制路径替换为当前仓库的实际路径(仓库迁移后重跑本脚本即可)。
#
#   ./install-service.sh            # 安装 + daemon-reload
#   ./install-service.sh --enable   # 安装 + 立即开启热点 + 开机自启
#   ./install-service.sh --uninstall
set -euo pipefail

REPO_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
SERVICE_SRC="$REPO_DIR/hotspotctl.service"
SERVICE_DST=/etc/systemd/system/hotspotctl.service
BIN="$REPO_DIR/target/release/hotspotctl"

MODE=install
case "${1:-}" in
  --enable) MODE=enable ;;
  --uninstall) MODE=uninstall ;;
  "") ;;
  *) echo "用法: $0 [--enable|--uninstall]" >&2; exit 2 ;;
esac

# 需要 root: 非 root 时用 run0 重新以 root 执行自身(本机约定: 不用 sudo)。
if [ "$(id -u)" -ne 0 ]; then
  exec run0 -- "$0" "$@"
fi

if [ "$MODE" = uninstall ]; then
  systemctl disable --now hotspotctl 2>/dev/null || true
  rm -f "$SERVICE_DST"
  systemctl daemon-reload
  echo "已卸载 hotspotctl.service"
  exit 0
fi

if [ ! -x "$BIN" ]; then
  echo "找不到 $BIN,请先在仓库根目录执行: cargo build --release" >&2
  exit 1
fi

# 用实际路径重写 service 文件(开发机路径 → 当前仓库路径)。
sed -e "s|^Documentation=.*|Documentation=file://$REPO_DIR/README.md|" \
    -e "s|^WorkingDirectory=.*|WorkingDirectory=$REPO_DIR|" \
    -e "s|^ExecStart=.*|ExecStart=$BIN on|" \
    -e "s|^ExecStop=.*|ExecStop=$BIN off|" \
    "$SERVICE_SRC" > "$SERVICE_DST"

systemctl daemon-reload
echo "已安装 $SERVICE_DST"

if [ "$MODE" = enable ]; then
  systemctl enable --now hotspotctl
  echo "已开启热点并设置开机自启 (systemctl status hotspotctl)"
else
  echo "启动热点: systemctl start hotspotctl  |  开机自启: systemctl enable hotspotctl"
fi
