# hotspotctl —— 便携热点管理器 · 项目说明 (Rust)

> 本文档是项目的**独立说明**。2026-08 曾用 Python(Textual + argparse)实现并沉淀了完整排障
> 经验,2026-09 用 Rust(ratatui + clap)全功能重写为单二进制,行为与 Python 版完全对齐。
> Python 版历史完整保留在 jj 父提交(`main`,wzvtrqpm)中,可用 `jj` 回退查看。

## 是什么

针对 CachyOS(NetworkManager + UFW)环境的一键热点管理工具:
**Rust 单二进制,TUI + CLI 双模式**,把实际排障中踩过的三类坑固化成了自动修复:

| 坑 | 症状 | hotspotctl 的修复 |
|---|---|---|
| rfkill 软屏蔽 | Wi-Fi「不可用」、热点入口消失 | `rfkill unblock wlan` |
| UFW `deny (incoming)` 丢弃 DHCP | 手机卡在「获取 IP」 | 放行 67/udp 与热点子网 |
| `net.ipv4.ip_forward=0` | 拿到 IP 也无法上网 | 开启并持久化转发 |

所有修复幂等（已存在则跳过）、持久化（重启有效）。

## 技术栈

- **Rust 1.97** / Cargo,`cargo build --release` 得到单一静态二进制(无需运行时依赖)
- **clap** CLI 子命令; **ratatui + crossterm** 全屏 TUI(三面板 + 热键 + 模态配置表单)
- **regex** 解析系统命令输出
- 核心依赖系统工具: `nmcli` / `ufw` / `iw` / `rfkill` / `sysctl`
- 代码结构: 检测函数(`detect.rs`，CLI/TUI 共用) → `status::collect_status()` 采集 → `actions::do_*()` 动作(无 exit) → `cli::cmd_*()` 包装 / `tui::App` 界面

## 快速上手

```bash
cargo build --release
ln -s "$PWD/target/release/hotspotctl" ~/.local/bin/hotspotctl

hotspotctl            # 交互终端 → TUI（默认）；管道 → status 文本
hotspotctl tui        # 显式进入 TUI (o 开 f 关 r 修 c 配置 s 刷新 p 密码 q 退出)
hotspotctl on         # 开热点: 自动修复 + 首次自动创建(随机密码) + 显示密码
hotspotctl on --ssid MyHotspot -p 12345678   # 首建时指定名称密码
hotspotctl off        # 关闭
hotspotctl config --ssid 新名                 # 改网络名称
hotspotctl config -p 新密码                   # 改密码（≥8位）
hotspotctl config --band 2.4                  # AP 频段: auto / 2.4 / 5g
hotspotctl config --security wpa2             # 安全算法: wpa2 / wpa3 / open
hotspotctl devices    # 在线设备: MAC/IP/主机名/信号/上下行速率
hotspotctl fix        # 只体检修复, 不开关热点
```

TUI 热键: `o` 开启 `f` 关闭 `r` 修复 `c` 配置(表单) `s` 刷新 `p` 密码显示/隐藏 `q` 退出。
配置表单(居中): 网络名称 / 密码 / AP 频段(自动·2.4G·5G) / 安全算法(WPA2·WPA3·开放);
`Enter` 保存 / `Esc` 取消 / `Tab`·`↑↓` 切换输入 / `←→` 切换选项。
状态与设备每 2s 自动刷新(后台线程采集,不阻塞界面);动作在独立线程执行。

## 权限

需要 root。非 root 时自动通过 `run0` 以 root 重执行自身,
Hyprland 桌面会弹 polkit 密码窗,输入一次即可。(符合本机「用 run0 不用 sudo」约定)

> 在非交互 agent 环境下 `run0` 可能偶发 `transient service 超时`,
> 属 systemd 瞬态单元在无 TTY 下的限制;在普通终端/桌面使用稳定。

## 用法详解

- **首次开启(on)**: 若尚无 AP 模式连接,自动创建(默认 SSID `CachyOS-Hotspot`,密码随机生成并打印)。
- **找热点连接**: `nmcli -g 802-11-wireless.mode con show <每连接>` 找 `mode=ap` 的那个。
  单字段查询必须用 `-g` 而非 `-t -f`(否则带字段名前缀无法比对)。
- **上游接口探测**: `ip route get 1.1.1.1` 取默认路由的 `dev`,作为转发目标。
- **密码显示**: NM 对 `psk` 查询返回 `<hidden>`,回退读取 keyfile 明文(`psk=` 行)。

## 维护与扩展

- 上游接口、Wi-Fi 接口均为运行时探测;`WLAN_DEFAULT` / `SUBNET` / 连接名是常量可调(`detect.rs`)
- 若迁移 firewalld: 替换 `actions::fix_ufw()` 中 `ufw ...` 为对应 `firewall-cmd --add-*`
- 若用 hostapd 手动方案(非 NM): 保留 `fix_rfkill` / `fix_ip_forward` 即可
- 解析逻辑集中在 `detect.rs` 并带单元测试,新增系统命令在此扩展

## 变更与 Bug 记录

- UFW 输出用 `ALLOW` 而非 `ALLOW IN`,且列间多空格 → 按行+特征串匹配
- `nmcli -t -f` 单字段带前缀 → 改 `-g`
- `channel 0` 非法 → 空字符串
- `nmcli` 隐藏 psk → keyfile 兜底
