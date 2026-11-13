//! 状态检测: 解析 nmcli / iw / rfkill / ufw / sysctl 输出为结构化数据。
//! 供 CLI 与 TUI 共用。

use std::collections::HashMap;
use std::fs;

use crate::run::run;

#[allow(dead_code)]
pub const NM_CONN_DEFAULT: &str = "Hotspot";
pub const SUBNET: &str = "10.42.0.0/24";
pub const SYSCTL_CONF: &str = "/etc/sysctl.d/99-hotspot.conf";
const LEASE_DIR: &str = "/var/lib/NetworkManager";

/// 在线设备(解析自 iw station dump + dnsmasq 租约)。
#[derive(Clone, Debug, Default)]
pub struct Station {
    pub mac: String,
    pub signal: Option<i32>,
    pub tx: Option<f64>,
    pub rx: Option<f64>,
}

/// 无线网卡接口名。
/// 优先 nmcli; nmcli 不可用/未就绪时回退到 /sys/class/net 里的 wl* 设备。
/// 探测失败返回 Err —— 绝不臆测 "wlan0": 那只是内核初始名, 开机瞬间就会被
/// udev 按可预测命名策略改成 wlpXsY, 用错会把规则写到不存在的接口上。
pub fn wlan_iface() -> Result<String, String> {
    let p = run("nmcli", &["-g", "DEVICE,TYPE", "dev", "status"]);
    for line in p.stdout.lines() {
        if let Some((dev, typ)) = line.split_once(':') {
            if typ == "wifi" && !dev.is_empty() {
                return Ok(dev.to_string());
            }
        }
    }
    if let Some(dev) = sysfs_wlan() {
        return Ok(dev);
    }
    Err("未找到无线网卡 (nmcli 无 wifi 设备, /sys/class/net 下也无 wl* 接口)".to_string())
}

/// /sys/class/net 里的无线接口(跳过 p2p-dev-* 这类虚拟子接口)。
fn sysfs_wlan() -> Option<String> {
    let mut names: Vec<String> = fs::read_dir("/sys/class/net")
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("wl"))
        .collect();
    names.sort();
    names.into_iter().next()
}

/// 从 `ip route get` / `ip route show` 输出里取出口设备名。
pub fn parse_route_dev(text: &str) -> Option<String> {
    let re = regex::Regex::new(r"\bdev\s+(\S+)").unwrap();
    re.captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// 是否为 TUN/TAP/WireGuard 这类无链路层类型的虚拟接口。
/// mihomo(Clash.Meta) 的 TUN 同时带 tun_flags 与 type=65534。
pub fn is_virtual_tun(dev: &str) -> bool {
    if dev.is_empty() {
        return false;
    }
    let base = format!("/sys/class/net/{dev}");
    if fs::metadata(format!("{base}/tun_flags")).is_ok() {
        return true;
    }
    fs::read_to_string(format!("{base}/type"))
        .map(|t| t.trim() == "65534")
        .unwrap_or(false)
}

/// `ip route get 1.1.1.1` 命中的出口。开着 TUN 代理时, 这里是被劫持的虚拟接口。
pub fn default_route_dev() -> Option<String> {
    parse_route_dev(&run("ip", &["route", "get", "1.1.1.1"]).stdout)
}

/// 物理上游接口(热点 NAT 的真实出口)。
/// 默认路由被 TUN 代理劫持时, 回退到 main 表的默认路由。
pub fn upstream_iface() -> String {
    let dev = default_route_dev().unwrap_or_default();
    if !dev.is_empty() && !is_virtual_tun(&dev) {
        return dev;
    }
    let p = run("ip", &["-4", "route", "show", "table", "main", "default"]);
    parse_route_dev(&p.stdout).unwrap_or(dev)
}

/// TUN 代理接口(如 Meta / tun0); 未启用 TUN 时为 None。
pub fn tun_iface() -> Option<String> {
    default_route_dev().filter(|d| is_virtual_tun(d))
}

/// UFW 规则类别。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UfwRuleKind {
    /// 热点口上的 DHCP (67/udp)
    Dhcp,
    /// 热点子网 → 本机服务(含 dnsmasq 的 53/udp)
    Subnet,
    /// 热点口 → 指定出口的转发
    Forward,
}

/// 判断 `ufw status` 文本里是否已有对应规则。
/// 必须同时校验接口名: 只比对子网串会让改名前的旧规则"顶替"新规则, 导致漏加。
pub fn ufw_has_rule(st: &str, kind: UfwRuleKind, wlan: &str, out: Option<&str>) -> bool {
    let wlan = wlan.trim();
    if wlan.is_empty() {
        return false;
    }
    match kind {
        UfwRuleKind::Dhcp => st.lines().any(|l| l.contains(&format!("67/udp on {wlan}"))),
        UfwRuleKind::Subnet => st.lines().any(|l| l.contains(SUBNET) && l.contains(wlan)),
        UfwRuleKind::Forward => out
            .filter(|o| !o.is_empty())
            .map(|o| {
                st.lines()
                    .any(|l| l.contains("ALLOW FWD") && l.contains(wlan) && l.contains(o))
            })
            .unwrap_or(false),
    }
}

/// 找到 NM 中的 AP 模式连接名。
/// 可能存在多个 AP 连接(如绑定在旧接口名上的残留连接), 优先选择
/// connection.interface-name 与当前 wifi 接口一致(或未绑定接口)的连接,
/// 避免激活时因 "mismatching interface name" 失败。
pub fn find_ap_conn() -> Option<String> {
    let wlan = wlan_iface().unwrap_or_default();
    let p = run("nmcli", &["-g", "NAME,TYPE", "con", "show"]);
    let mut fallback = None;
    for line in p.stdout.lines() {
        let mut it = line.splitn(2, ':');
        let name = it.next().unwrap_or("");
        let typ = it.next().unwrap_or("");
        if typ != "802-11-wireless" {
            continue;
        }
        let m = run(
            "nmcli",
            &["-g", "802-11-wireless.mode", "con", "show", name],
        );
        if m.stdout.trim() != "ap" {
            continue;
        }
        let bound = run(
            "nmcli",
            &["-g", "connection.interface-name", "con", "show", name],
        )
        .stdout
        .trim()
        .to_string();
        if bound.is_empty() || (!wlan.is_empty() && bound == wlan) {
            return Some(name.to_string());
        }
        if fallback.is_none() {
            fallback = Some(name.to_string());
        }
    }
    fallback
}

/// (wlan接口, 状态, 连接名)
pub fn hotspot_state() -> (Option<String>, Option<String>, Option<String>) {
    let p = run(
        "nmcli",
        &["-g", "DEVICE,TYPE,STATE,CONNECTION", "dev", "status"],
    );
    for line in p.stdout.lines() {
        let mut it = line.splitn(4, ':');
        let dev = it.next().unwrap_or("").to_string();
        let typ = it.next().unwrap_or("");
        let state = it.next().unwrap_or("").to_string();
        let conn = it.next().unwrap_or("").to_string();
        if typ == "wifi" {
            return (Some(dev), Some(state), Some(conn));
        }
    }
    (None, None, None)
}

/// (ssid, type, channel 文本)
pub fn ap_info(wlan: &str) -> (Option<String>, Option<String>, Option<String>) {
    let p = run("iw", &["dev", wlan, "info"]);
    let mut ssid = None;
    let mut typ = None;
    let mut ch = None;
    for line in p.stdout.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("ssid ") {
            ssid = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("type ") {
            typ = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("channel ") {
            ch = Some(rest.trim().to_string());
        }
    }
    (ssid, typ, ch)
}

/// (soft_blocked, hard_blocked)
pub fn rfkill_status() -> (Option<bool>, Option<bool>) {
    let p = run("rfkill", &["list"]);
    for block in p.stdout.split("\n\n") {
        let lower = block.to_lowercase();
        if block.contains("Wireless LAN") || lower.contains("wlan") {
            return (
                Some(block.contains("Soft blocked: yes")),
                Some(block.contains("Hard blocked: yes")),
            );
        }
    }
    (None, None)
}

pub fn ip_forward_enabled() -> bool {
    fs::read_to_string("/proc/sys/net/ipv4/ip_forward")
        .map(|s| s.trim() == "1")
        .unwrap_or(false)
}

pub fn ufw_status_text() -> String {
    run("ufw", &["status"]).stdout
}

/// 读取 dnsmasq 租约: mac -> (ip, hostname)
pub fn get_leases() -> HashMap<String, (String, String)> {
    let mut leases = HashMap::new();
    let Ok(entries) = fs::read_dir(LEASE_DIR) else {
        return leases;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("dnsmasq-") || !name.ends_with(".leases") {
            continue;
        }
        let Ok(contents) = fs::read_to_string(entry.path()) else {
            continue;
        };
        for line in contents.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                leases.insert(
                    parts[1].to_string(),
                    (parts[2].to_string(), parts[3].to_string()),
                );
            }
        }
    }
    leases
}

/// 解析 iw station dump → Vec<Station>
pub fn parse_stations(wlan: &str) -> Vec<Station> {
    let p = run("iw", &["dev", wlan, "station", "dump"]);
    parse_stations_text(&p.stdout)
}

/// 解析 iw station dump 文本(可单测)。
pub fn parse_stations_text(text: &str) -> Vec<Station> {
    let re_sig = regex::Regex::new(r"signal:\s+(-?\d+)").unwrap();
    let re_tx = regex::Regex::new(r"tx bitrate:\s+([\d.]+)\s+MBit/s").unwrap();
    let re_rx = regex::Regex::new(r"rx bitrate:\s+([\d.]+)\s+MBit/s").unwrap();

    let mut stations = Vec::new();
    let mut cur: Option<Station> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Station ") {
            let mac = rest
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches(')')
                .to_string();
            if let Some(prev) = cur.take() {
                stations.push(prev);
            }
            cur = Some(Station {
                mac,
                signal: None,
                tx: None,
                rx: None,
            });
        } else if let Some(st) = cur.as_mut() {
            if let Some(cap) = re_sig.captures(line) {
                st.signal = cap.get(1).and_then(|m| m.as_str().parse::<i32>().ok());
            } else if let Some(cap) = re_tx.captures(line) {
                st.tx = cap.get(1).and_then(|m| m.as_str().parse::<f64>().ok());
            } else if let Some(cap) = re_rx.captures(line) {
                st.rx = cap.get(1).and_then(|m| m.as_str().parse::<f64>().ok());
            }
        }
    }
    if let Some(prev) = cur.take() {
        stations.push(prev);
    }
    stations
}

/// 信号强度分级: dBm → 强/良好/一般/弱
pub fn sig_label(signal: Option<i32>) -> &'static str {
    match signal {
        None => "-",
        Some(s) if s >= -50 => "强",
        Some(s) if s >= -65 => "良好",
        Some(s) if s >= -75 => "一般",
        Some(_) => "弱",
    }
}

/// 读取热点密码: nmcli 优先, 隐藏时回退 keyfile 明文。
pub fn get_password(conn: &str) -> Option<String> {
    let p = run(
        "nmcli",
        &["-g", "802-11-wireless-security.psk", "con", "show", conn],
    );
    let s = p.stdout.trim();
    if !s.is_empty() && s != "<hidden>" {
        return Some(s.to_string());
    }
    let re = regex::Regex::new(r"(?m)^psk=(.+)$").unwrap();
    for path in [
        format!("/etc/NetworkManager/system-connections/{conn}.nmconnection"),
        format!("/etc/NetworkManager/system-connections/{conn}"),
    ] {
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        if let Some(cap) = re.captures(&contents) {
            return cap.get(1).map(|m| m.as_str().to_string());
        }
    }
    None
}

/// TUI 用: 读取网络名称。
pub fn get_ssid(conn: &str) -> String {
    run(
        "nmcli",
        &["-g", "802-11-wireless.ssid", "con", "show", conn],
    )
    .stdout
    .trim()
    .to_string()
}

/// TUI 用: 读取 AP 频段 (bg / a / 空=auto)。
pub fn get_band(conn: &str) -> String {
    run(
        "nmcli",
        &["-g", "802-11-wireless.band", "con", "show", conn],
    )
    .stdout
    .trim()
    .to_string()
}

/// TUI 用: 读取安全算法 (wpa-psk / sae / 空=开放)。
pub fn get_key_mgmt(conn: &str) -> String {
    run(
        "nmcli",
        &[
            "-g",
            "802-11-wireless-security.key-mgmt",
            "con",
            "show",
            conn,
        ],
    )
    .stdout
    .trim()
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_stations() {
        let sample = "Station 12:34:56:78:9a:bc (on wlan0)\n\
            \tInactive time:\t120 ms\n\
            \trx bitrate:\t866.7 MBit/s\n\
            \tsignal:\t-45.0 dBm\n\
            \ttx bitrate:\t433.3 MBit/s\n\
            Station de:ad:be:ef:00:11 (on wlan0)\n\
            \tsignal:\t-70.0 dBm\n";
        let st = parse_stations_text(sample);
        assert_eq!(st.len(), 2);
        assert_eq!(st[0].mac, "12:34:56:78:9a:bc");
        assert_eq!(st[0].signal, Some(-45));
        assert_eq!(st[0].tx, Some(433.3));
        assert_eq!(st[0].rx, Some(866.7));
        assert_eq!(st[1].mac, "de:ad:be:ef:00:11");
        assert_eq!(st[1].signal, Some(-70));
        assert_eq!(st[1].tx, None);
    }

    #[test]
    fn test_sig_label() {
        assert_eq!(sig_label(None), "-");
        assert_eq!(sig_label(Some(-40)), "强");
        assert_eq!(sig_label(Some(-60)), "良好");
        assert_eq!(sig_label(Some(-70)), "一般");
        assert_eq!(sig_label(Some(-80)), "弱");
        assert_eq!(sig_label(Some(-50)), "强");
        assert_eq!(sig_label(Some(-65)), "良好");
        assert_eq!(sig_label(Some(-75)), "一般");
    }

    #[test]
    fn test_parse_route_dev() {
        // 普通默认路由。
        assert_eq!(
            parse_route_dev("1.1.1.1 via 192.168.1.1 dev enp44s0 src 192.168.1.205 uid 0"),
            Some("enp44s0".to_string())
        );
        // 被 TUN 代理劫持。
        assert_eq!(
            parse_route_dev("1.1.1.1 via 198.18.0.2 dev Meta table 2022 src 198.18.0.1"),
            Some("Meta".to_string())
        );
        // 无路由。
        assert_eq!(parse_route_dev("RTNETLINK answers: Network is unreachable"), None);
    }

    #[test]
    fn test_ufw_has_rule_is_iface_aware() {
        // 换过接口名的现场: 旧规则绑 wlan0, 新接口是 wlp0s20f3。
        let st = "\
Status: active

To                         Action      From
--                         ------      ----
67/udp on wlan0            ALLOW       Anywhere
10.42.0.0/24               ALLOW IN    Anywhere on wlan0
Anywhere on wlan0          ALLOW FWD   Anywhere on enp44s0
67/udp on wlp0s20f3        ALLOW       Anywhere
10.42.0.0/24               ALLOW IN    Anywhere on wlp0s20f3
Anywhere on wlp0s20f3      ALLOW FWD   Anywhere on Meta
";
        // 新接口: 三类规则都该判定为已存在, 不能被子网串误判。
        assert!(ufw_has_rule(st, UfwRuleKind::Dhcp, "wlp0s20f3", None));
        assert!(ufw_has_rule(st, UfwRuleKind::Subnet, "wlp0s20f3", None));
        assert!(ufw_has_rule(st, UfwRuleKind::Forward, "wlp0s20f3", Some("Meta")));
        // 出口不同则必须判为缺失(旧逻辑只看入接口, 会把 out enp44s0 当成已放行)。
        assert!(!ufw_has_rule(st, UfwRuleKind::Forward, "wlp0s20f3", Some("enp44s0")));
        // 旧接口名不应再被认为覆盖了新接口。
        assert!(!ufw_has_rule(st, UfwRuleKind::Subnet, "wlp9s9f9", None));
        // 空接口名一律视为缺失, 避免写出 "in on  ..." 这种垃圾规则。
        assert!(!ufw_has_rule(st, UfwRuleKind::Dhcp, "", None));
    }

    #[test]
    fn test_psk_line() {
        let contents = "psk=MySecretPw123\nkey-mgmt=wpa-psk\n";
        let re = regex::Regex::new(r"(?m)^psk=(.+)$").unwrap();
        assert_eq!(
            re.captures(contents).unwrap().get(1).unwrap().as_str(),
            "MySecretPw123"
        );
    }
}
