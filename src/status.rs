//! 状态汇总: 一次性采集全部状态, 供 CLI status / TUI 共用。

use std::collections::HashMap;

use crate::detect::Station;

#[derive(Clone, Debug)]
pub struct UfwRule {
    pub desc: String,
    pub found: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub wlan: Option<String>,
    pub state: Option<String>,
    pub conn: Option<String>,
    pub ssid: Option<String>,
    pub typ: Option<String>,
    pub channel: Option<String>,
    pub password: Option<String>,
    pub soft_block: Option<bool>,
    pub hard_block: Option<bool>,
    pub ip_forward: bool,
    pub ufw_active: bool,
    pub ufw_rules: Vec<UfwRule>,
    pub leases: HashMap<String, (String, String)>,
    pub stations: Vec<Station>,
}

impl Status {
    /// 热点是否处于运行中。
    pub fn running(&self) -> bool {
        self.conn.as_deref().is_some()
            && matches!(self.state.as_deref(), Some("connected") | Some("已连接"))
    }

    /// 射频是否未被屏蔽。
    pub fn rf_ok(&self) -> bool {
        !(self.soft_block == Some(true) || self.hard_block == Some(true))
    }
}

/// 一次性采集全部状态。
pub fn collect_status() -> Status {
    use crate::detect::*;

    let (wlan, state, conn) = hotspot_state();
    let (ssid, typ, channel) = match &wlan {
        Some(w) => ap_info(w),
        None => (None, None, None),
    };
    let (soft_block, hard_block) = rfkill_status();
    let ip_forward = ip_forward_enabled();
    let ufw_text = ufw_status_text();
    let ufw_active = ufw_text.contains("Status: active");

    // 匹配 UFW 规则 (按行+特征串, 与脚本一致)。
    let mut rules = Vec::new();
    let wlan_str = wlan.as_deref().unwrap_or(WLAN_DEFAULT);
    for (needle, desc) in [
        (format!("67/udp on {wlan_str}"), "DHCP 放行"),
        (format!("ALLOW IN {SUBNET}"), "子网放行"),
        ("ALLOW FWD".to_string(), "转发放行"),
    ] {
        let found = if needle.contains(SUBNET) {
            ufw_text.lines().any(|l| l.contains(SUBNET))
        } else if needle.starts_with("ALLOW") {
            ufw_text
                .lines()
                .any(|l| l.contains("ALLOW FWD") && l.contains(wlan_str))
        } else {
            ufw_text.contains(&needle)
        };
        rules.push(UfwRule {
            desc: desc.to_string(),
            found,
        });
    }

    let leases = get_leases();
    let stations = match &wlan {
        Some(w) => parse_stations(w),
        None => Vec::new(),
    };

    let password = conn.as_deref().and_then(get_password);

    Status {
        wlan,
        state,
        conn,
        ssid,
        typ,
        channel,
        password,
        soft_block,
        hard_block,
        ip_forward,
        ufw_active,
        ufw_rules: rules,
        leases,
        stations,
    }
}
