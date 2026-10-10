# hotspotctl — Linux 便携热点管理器 (Rust)

针对 CachyOS / NetworkManager / UFW 环境的一键热点管理工具。
纯 Rust 单二进制(ratatui TUI + clap CLI 双模式),把实际排障中遇到的所有问题固化成了自动修复:
**rfkill 屏蔽、UFW 拦截 DHCP、内核转发未开启**。

## 为什么需要它

Linux 本身支持热点(NetworkManager + hostapd 底层),但有三道常见的坑会导致
「热点能开、手机连不上 / 卡在获取 IP」:

| 坑 | 症状 | 本工具的修复 |
|---|---|---|
| rfkill 软屏蔽 | Wi-Fi 显示「不可用」，热点入口消失 | `rfkill unblock wlan` |
| UFW `deny (incoming)` 丢弃 DHCP | 手机卡在「获取 IP 地址」 | 放行 67/udp 与热点子网 |
| `ip_forward=0` | 拿到 IP 也无法共享上网 | 开启并持久化转发 |

`hotspotctl on` 会自动检测并修复以上所有问题，然后开启热点。

## 构建与安装

```bash
# 构建(需 Rust 工具链)
cargo build --release        # 产物: target/release/hotspotctl

# 加入 PATH(可选, 之后任意目录可调)
ln -s "$PWD/target/release/hotspotctl" ~/.local/bin/hotspotctl
```

### 免构建安装(下载预编译产物)

每次推送 `v*` tag 都会由 GitHub Actions 自动构建并发布到 Releases:

```bash
# Arch Linux: 直接装包(自带 /usr/bin/hotspotctl 与 systemd 单元)
pacman -U hotspotctl-0.1.0-1-x86_64.pkg.tar.zst

# 其他发行版: 下载裸二进制(校验完整性后放入 PATH)
sha256sum -c ALL_SHA256SUMS --ignore-missing
install -Dm755 hotspotctl-x86_64-linux ~/.local/bin/hotspotctl
```

> 二进制为动态链接的 x86_64 ELF,依赖 glibc 与系统工具 `nmcli`/`ufw`/`iw`/`rfkill`。
> Arch 包在 `archlinux:base-devel` 容器中由 `makepkg` 构建,不依赖本机环境。

## systemd 服务管理

仓库内自带 `hotspotctl.service`(一次性服务)与 `install-service.sh`,可把热点开关接进 systemd:

```bash
./install-service.sh            # 安装到 /etc/systemd/system/ 并 daemon-reload
./install-service.sh --enable   # 安装 + 立即开启 + 开机自启
```

```bash
systemctl start   hotspotctl   # 开启热点 (= hotspotctl on, 含自动修复)
systemctl stop    hotspotctl   # 关闭热点 (= hotspotctl off)
systemctl enable  hotspotctl   # 开机自启热点
systemctl status  hotspotctl   # 查看状态与最近日志
journalctl -u hotspotctl -f    # 跟踪热点日志
```

服务以 root 运行,无需 run0 提权。`install-service.sh` 会把 service 文件里的仓库路径替换为目标
路径并指向 `target/release/hotspotctl`(需先 `cargo build --release`),仓库迁移后重跑即可。

## TUI 界面（默认）

在交互终端直接运行 `hotspotctl`(或 `hotspotctl tui`)进入全屏界面，
后台线程每 2 秒自动刷新状态与在线设备:

| 热键 | 功能 |
|---|---|
| `o` | 开启热点（自动修复 rfkill/防火墙/转发） |
| `f` | 关闭热点 |
| `r` | 一键修复 |
| `c` | 配置 网络名称 / 密码 / AP频段 / 安全算法（居中表单） |
| `s` | 手动刷新 |
| `p` | 切换密码显示（掩码 `*` ⇄ 明文） |
| `q` | 退出 |

配置表单内:`Enter` 保存 / `Esc` 取消 / `Tab` 或 `↑↓` 切换输入 / `←→` 在频段与安全算法间切换选项。

状态面板显示:网络名称、密码（默认掩码，`p` 切换）、接口、信道、射频、转发、防火墙规则。
设备面板实时列出在线设备(MAC/IP/主机名/信号/上下行速率),底部日志面板滚动展示历史动作。

非交互(管道/脚本)下无参数运行仍输出 `status` 文本,兼容原有用法。

## CLI 用法

```bash
hotspotctl status     # 状态总览: 连接/射频/防火墙/转发/在线设备
hotspotctl on         # 开启热点(自动修复, 首次自动创建 SSID 和随机密码)
hotspotctl on --ssid MyHotspot -p 12345678   # 首次创建时指定名称密码
hotspotctl off        # 关闭热点
hotspotctl config --ssid 新名称               # 修改网络名称
hotspotctl config -p 新密码                   # 修改密码(≥8位)
hotspotctl config --band 2.4                  # AP 频段: auto / 2.4 / 5g
hotspotctl config --security wpa2             # 安全算法: wpa2 / wpa3 / open
hotspotctl devices    # 列出已连接设备: MAC/IP/主机名/信号/速率
hotspotctl fix        # 只做体检修复, 不开关热点
hotspotctl tui        # 显式进入 TUI 界面
```

> 无参数时: 交互终端 → 进入 TUI; 管道/脚本 → 输出 status 文本。

## 工作原理

```
hotspotctl on
  ├─ fix_rfkill()      # 检查/解除 rfkill 软屏蔽
  ├─ fix_ip_forward()  # 写入 /etc/sysctl.d/99-hotspot.conf + sysctl -w
  ├─ fix_ufw()         # 幂等添加 3 条 UFW 规则:
  │   ├─ ufw allow in on wlan0 to any port 67 proto udp   (DHCP)
  │   ├─ ufw allow in on wlan0 from 10.42.0.0/24          (子网)
  │   └─ ufw route allow in on wlan0 out on <上游接口>      (转发)
  └─ nmcli connection up <热点连接>   # 找不到时自动创建
```

- NAT/转发由 NetworkManager 自动管理(`nm-shared-wlan0` 表,MASQUERADE + FORWARD)
- UFW 规则、sysctl 配置均持久化,重启后仍有效
- 上游接口自动检测(`ip route get 1.1.1.1`),换网卡不用改配置
- 非 root 时通过 `run0` 自动以 root 重执行自身(桌面弹 polkit 密码窗)

## 代码结构

```
src/
  main.rs     入口: run0 提权 + clap 分派 + 无参数入口策略
  run.rs      外部命令助手(LANG=C + 30s 超时)
  log.rs      日志层(CLI 打印 / TUI 事件缓冲)
  detect.rs   解析 nmcli/iw/rfkill/ufw/sysctl 输出
  status.rs   Status 结构 + collect_status()
  actions.rs  fix_* / do_on / do_off / do_config / do_fix
  cli.rs      clap 子命令定义 + cmd_* 包装
  tui.rs      ratatui 三面板主屏 + 配置模态表单 + 热键
```

`cargo test` 含解析逻辑单元测试(parse_stations/sig_label 等)。

## 常见问题

- **重启后 Wi-Fi 又「不可用」**：rfkill 软屏蔽在部分笔记本(如联想)上会随开机重置，
  再跑一次 `hotspotctl fix` 或 `hotspotctl on` 即可；可考虑 BIOS 里关闭飞行模式。
- **手机连上但网速慢**：信道自动选择可能落在拥挤信道，用 `config --band 2.4` 固定到 2.4G。
- **改配置后不生效**：工具会自动重启热点连接；若手动改过 NM 配置，需 down/up 一次。
