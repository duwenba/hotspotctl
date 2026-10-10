## 变更 (v0.1.0 → v0.1.1)

- release: v0.1.1 (版本号 0.1.0 -> 0.1.1, README 产物名改用通配) (`f1b5aa9`)
- docs: 增加 AI/LLM 使用声明(强免责) (`d133fcf`)

**完整变更**: https://github.com/duwenba/hotspotctl/compare/v0.1.0...v0.1.1

## 安装

```bash
sha256sum -c ALL_SHA256SUMS
pacman -U hotspotctl-*.pkg.tar.zst                        # Arch (含 systemd 单元)
install -Dm755 hotspotctl-x86_64-linux ~/.local/bin/hotspotctl   # 其他发行版
```

## ⚠️ AI 使用声明

**本项目代码主要由 AI 生成，作者仅做集成与测试。不对正确性、安全性作任何担保；使用风险自负。**

本工具会以 **root 权限修改系统状态**（`rfkill`、`/etc/sysctl.d/*.conf`、UFW 防火墙规则、NetworkManager 连接配置），
请先阅读仓库 README 中的完整声明与源码后再决定是否使用。
