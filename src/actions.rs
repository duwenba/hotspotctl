//! 修复/动作: fix_rfkill / fix_ip_forward / fix_ufw / do_on / do_off / do_fix / do_config。
//! 不直接 exit, 返回 Result/bool, 由 CLI/TUI 决定后续。

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};

use crate::detect::{self, SYSCTL_CONF};
use crate::log;
use crate::run::run;

/// 随机生成 n 字节的 hex 字符串(对应 secrets.token_hex)。
fn random_hex(nbytes: usize) -> String {
    let mut f = File::open("/dev/urandom").expect("无法打开 /dev/urandom");
    let mut buf = vec![0u8; nbytes];
    f.read_exact(&mut buf).expect("读取 /dev/urandom 失败");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// 修复 rfkill。hard 屏蔽不可解 → Err; soft 解除 → 成功。
pub fn fix_rfkill() -> Result<(), String> {
    let (soft, hard) = detect::rfkill_status();
    if hard == Some(true) {
        log::fail("无线网卡被【硬件】屏蔽, 请检查笔记本飞行模式开关或 BIOS 设置");
        return Err("射频被硬件屏蔽".to_string());
    }
    if soft == Some(true) {
        log::warn("无线网卡被软屏蔽(rfkill), 解除中...");
        run("rfkill", &["unblock", "wlan"]).check(&["rfkill", "unblock", "wlan"])?;
        log::ok("已解除软屏蔽");
    } else {
        log::ok("射频状态正常");
    }
    Ok(())
}

/// 开启并持久化内核 IP 转发。
pub fn fix_ip_forward() -> Result<(), String> {
    if detect::ip_forward_enabled() {
        log::ok("内核 IP 转发已开启");
        return Ok(());
    }
    log::warn("内核 IP 转发未开启, 开启并持久化...");
    let mut f = OpenOptions::new()
        .write(true)
        .truncate(true)
        .create(true)
        .open(SYSCTL_CONF)
        .map_err(|e| format!("写入 {SYSCTL_CONF} 失败: {e}"))?;
    f.write_all(b"net.ipv4.ip_forward=1\n")
        .map_err(|e| format!("写入失败: {e}"))?;
    run("sysctl", &["-w", "net.ipv4.ip_forward=1"]).check(&[
        "sysctl",
        "-w",
        "net.ipv4.ip_forward=1",
    ])?;
    log::ok(&format!("已开启 (持久化于 {SYSCTL_CONF})"));
    Ok(())
}

/// 计算某条 UFW 规则是否已存在(与 status.rs 同一套匹配逻辑)。
fn ufw_rule_exists(st: &str, needle: &str, wlan: &str) -> bool {
    let subnet = detect::SUBNET;
    if needle.contains(subnet) {
        st.lines().any(|l| l.contains(subnet))
    } else if needle.starts_with("ALLOW") {
        st.lines()
            .any(|l| l.contains("ALLOW FWD") && l.contains(wlan))
    } else {
        st.contains(needle)
    }
}

/// 幂等添加 UFW 规则。
pub fn fix_ufw(wlan: &str, up: &str) -> Result<(), String> {
    let mut st = detect::ufw_status_text();
    if !st.contains("Status: active") {
        log::warn("UFW 未启用, 启用...");
        run("ufw", &["enable"]).check(&["ufw", "enable"])?;
        st = detect::ufw_status_text();
    }
    let rules: [(&str, &str, &str); 3] = [
        (
            &format!("67/udp on {wlan}"),
            "allow",
            &format!("in on {wlan} to any port 67 proto udp"),
        ),
        (
            &format!("ALLOW IN {}", detect::SUBNET),
            "allow",
            &format!("in on {wlan} from {}", detect::SUBNET),
        ),
        (
            "ALLOW FWD",
            "route",
            &format!("allow in on {wlan} out on {up}"),
        ),
    ];
    for (needle, kind, rule) in rules {
        let exists = ufw_rule_exists(&st, needle, wlan);
        let desc = match needle {
            n if n.contains(detect::SUBNET) => "子网放行",
            n if n.starts_with("ALLOW") => "转发放行",
            _ => "DHCP 放行",
        };
        if exists {
            log::ok(&format!("{desc}: 已存在"));
        } else {
            log::warn(&format!("{desc}: 添加 UFW 规则..."));
            let argv: Vec<&str> = rule.split_whitespace().collect();
            let mut full: Vec<&str> = Vec::with_capacity(argv.len() + 1);
            full.push(kind);
            full.extend(argv);
            run("ufw", &full).check(&full)?;
            log::ok(&format!("{desc}: 已添加"));
        }
    }
    Ok(())
}

/// 开启热点, 返回是否成功。
pub fn do_on(ssid: Option<&str>, password: Option<&str>) -> bool {
    if fix_rfkill().is_err() {
        return false;
    }
    let _ = fix_ip_forward();
    let wlan = detect::wlan_iface();
    let up = detect::upstream_iface();
    let _ = fix_ufw(&wlan, &up);

    // NM shared 模式依赖 dnsmasq 提供 DHCP/DNS, 缺失时激活会以
    // "IP configuration could not be reserved" 之类晦涩错误失败, 提前拦截。
    if !run("sh", &["-c", "command -v dnsmasq"]).ok() {
        log::fail("未安装 dnsmasq (NetworkManager 热点模式必需), 请先安装: sudo pacman -S dnsmasq");
        return false;
    }

    let mut conn = detect::find_ap_conn();
    if conn.is_none() {
        let ssid = ssid.unwrap_or("CachyOS-Hotspot");
        let pwd = match password {
            Some(p) => p.to_string(),
            None => random_hex(5),
        };
        log::info(&format!("未找到热点连接, 创建 {ssid} (密码 {pwd})..."));
        let full = [
            "dev", "wifi", "hotspot", "ifname", &wlan, "ssid", ssid, "password", &pwd,
        ];
        if run("nmcli", &full).check(&full).is_err() {
            log::fail("创建热点连接失败");
            return false;
        }
        conn = detect::find_ap_conn();
    }
    let Some(conn) = conn else {
        log::fail("创建热点连接后仍未找到 AP 连接");
        return false;
    };

    let p = run("nmcli", &["connection", "up", &conn]);
    let p = if p.ok() {
        p
    } else if p.stderr.contains("mismatching interface name") {
        // 连接绑定在旧接口名上(如接口改名/残留连接), 重绑到当前接口后重试。
        log::warn("连接绑定的接口名与当前不符, 重新绑定后重试...");
        let fix = ["con", "modify", &conn, "connection.interface-name", &wlan];
        if run("nmcli", &fix).check(&fix).is_err() {
            log::fail("重新绑定接口名失败");
            return false;
        }
        run("nmcli", &["connection", "up", &conn])
    } else {
        p
    };
    if p.ok() {
        let (ssid, _typ, ch) = detect::ap_info(&wlan);
        log::ok(&format!(
            "热点已开启: {} ({})",
            ssid.as_deref().unwrap_or(""),
            ch.as_deref().unwrap_or("")
        ));
        if let Some(pwd) = detect::get_password(&conn) {
            if log::is_tui_mode() {
                log::info("密码已读取, 状态面板按 p 查看");
            } else {
                log::info(&format!("密码: {pwd}"));
            }
        }
        true
    } else {
        log::fail(&format!("开启失败: {}", p.stderr.trim()));
        false
    }
}

/// 关闭热点。
pub fn do_off() {
    let Some(conn) = detect::find_ap_conn() else {
        log::warn("未找到热点连接");
        return;
    };
    let p = run("nmcli", &["connection", "down", &conn]);
    if p.ok() {
        log::ok("热点已关闭");
    } else {
        log::fail(&format!("关闭失败: {}", p.stderr.trim()));
    }
}

/// 一键修复。
pub fn do_fix() {
    let ok_ = fix_rfkill().is_ok();
    let _ = fix_ip_forward();
    let _ = fix_ufw(&detect::wlan_iface(), &detect::upstream_iface());
    if ok_ {
        log::ok("全部检查通过, 可以开启热点");
    }
}

/// 修改热点配置, 返回是否成功。band: ''=auto / bg / a; key_mgmt: ''=开放 / wpa-psk / sae。
pub fn do_config(
    conn: &str,
    ssid: Option<&str>,
    password: Option<&str>,
    band: Option<&str>,
    key_mgmt: Option<&str>,
) -> bool {
    let mut changed = false;
    if let Some(ssid) = ssid {
        let full = ["con", "modify", conn, "802-11-wireless.ssid", ssid];
        if run("nmcli", &full).check(&full).is_err() {
            return false;
        }
        log::ok(&format!("网络名称 → {ssid}"));
        changed = true;
    }
    if let Some(password) = password {
        if password.chars().count() < 8 {
            log::fail("密码至少 8 位");
            return false;
        }
        let full = ["con", "modify", conn, "wifi-sec.psk", password];
        if run("nmcli", &full).check(&full).is_err() {
            return false;
        }
        log::ok("密码已更新");
        changed = true;
    }
    if let Some(band) = band {
        if band.is_empty() {
            let full = [
                "con",
                "modify",
                conn,
                "802-11-wireless.band",
                "",
                "802-11-wireless.channel",
                "",
            ];
            if run("nmcli", &full).check(&full).is_err() {
                return false;
            }
            log::ok("AP 频段 → 自动");
        } else {
            let full = [
                "con",
                "modify",
                conn,
                "802-11-wireless.band",
                band,
                "802-11-wireless.channel",
                "",
            ];
            if run("nmcli", &full).check(&full).is_err() {
                return false;
            }
            log::ok(&format!(
                "AP 频段 → {}",
                if band == "bg" { "2.4GHz" } else { "5GHz" }
            ));
        }
        changed = true;
    }
    if let Some(key_mgmt) = key_mgmt {
        if key_mgmt.is_empty() {
            let full = ["con", "modify", conn, "remove", "wifi-sec"];
            if run("nmcli", &full).check(&full).is_err() {
                return false;
            }
            log::ok("安全算法 → 开放 (无密码)");
        } else {
            let full = ["con", "modify", conn, "wifi-sec.key-mgmt", key_mgmt];
            if run("nmcli", &full).check(&full).is_err() {
                return false;
            }
            log::ok(&format!(
                "安全算法 → {}",
                if key_mgmt == "wpa-psk" {
                    "WPA2"
                } else {
                    "WPA3"
                }
            ));
        }
        changed = true;
    }
    if changed {
        log::info("重启连接使配置生效...");
        run("nmcli", &["connection", "down", conn]);
        run("nmcli", &["connection", "up", conn]);
        log::ok("已生效");
    }
    true
}
