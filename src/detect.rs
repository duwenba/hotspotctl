//! 状态检测: 解析 nmcli / iw / rfkill / ufw / sysctl 输出为结构化数据。
//! 供 CLI 与 TUI 共用。

use std::collections::HashMap;
use std::fs;

use crate::run::run;

#[allow(dead_code)]
pub const NM_CONN_DEFAULT: &str = "Hotspot";
pub const WLAN_DEFAULT: &str = "wlan0";
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

/// 第一个 wifi 接口名。
pub fn wlan_iface() -> String {
    let p = run("nmcli", &["-g", "DEVICE,TYPE", "dev", "status"]);
    for line in p.stdout.lines() {
        let mut it = line.splitn(2, ':');
        let dev = it.next().unwrap_or("");
        let typ = it.next().unwrap_or("");
        if typ == "wifi" {
            return dev.to_string();
        }
    }
    WLAN_DEFAULT.to_string()
}

/// 默认路由的上游接口(热点转发目标)。
pub fn upstream_iface() -> String {
    let p = run("ip", &["route", "get", "1.1.1.1"]);
    let re = regex::Regex::new(r"dev\s+(\S+)").unwrap();
    if let Some(cap) = re.captures(&p.stdout) {
        cap.get(1)
            .map(|m| m.as_str().to_string())
            .unwrap_or_default()
    } else {
        "enp44s0".to_string()
    }
}

/// 找到 NM 中的 AP 模式连接名。
/// 可能存在多个 AP 连接(如绑定在旧接口名上的残留连接), 优先选择
/// connection.interface-name 与当前 wifi 接口一致(或未绑定接口)的连接,
/// 避免激活时因 "mismatching interface name" 失败。
pub fn find_ap_conn() -> Option<String> {
    let wlan = wlan_iface();
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
        if bound.is_empty() || bound == wlan {
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
    for path in [
        format!("/etc/NetworkManager/system-connections/{conn}.nmconnection"),
        format!("/etc/NetworkManager/system-connections/{conn}"),
    ] {
        let Ok(contents) = fs::read_to_string(&path) else {
            continue;
        };
        let re = regex::Regex::new(r"(?m)^psk=(.+)$").unwrap();
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
    fn test_upstream_extract() {
        // 从 "xxx dev enp1s0 src ..." 提取 dev 后的接口名。
        let re = regex::Regex::new(r"dev\s+(\S+)").unwrap();
        let out = "1.1.1.1 via 192.168.1.1 dev enp44s0 src 192.168.1.5 uid 1000";
        assert_eq!(
            re.captures(out).unwrap().get(1).unwrap().as_str(),
            "enp44s0"
        );
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
