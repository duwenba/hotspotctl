//! CLI 层: clap 子命令定义 + 各 cmd_* 包装 + 无参数入口策略。

use clap::{Parser, Subcommand};

use crate::actions;
use crate::detect;
use crate::log;
use crate::status;

#[derive(Parser)]
#[command(
    name = "hotspotctl",
    version,
    about = "hotspotctl - Linux 便携热点管理器 (NetworkManager + UFW)",
    disable_help_subcommand = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// 查看热点与系统状态
    Status,
    /// 开启热点(自动修复常见问题)
    On {
        /// 热点名称(仅首次创建时生效)
        #[arg(long)]
        ssid: Option<String>,
        /// 热点密码(≥8位, 仅首次创建时生效)
        #[arg(short, long)]
        password: Option<String>,
    },
    /// 关闭热点
    Off,
    /// 修改热点配置
    Config {
        /// 新的网络名称
        #[arg(long)]
        ssid: Option<String>,
        /// 新的密码(≥8位)
        #[arg(short, long)]
        password: Option<String>,
        /// AP 频段: auto / 2.4 / 5g
        #[arg(long)]
        band: Option<String>,
        /// 安全算法: wpa2 / wpa3 / open
        #[arg(long)]
        security: Option<String>,
    },
    /// 列出已连接设备
    Devices,
    /// 一键修复 rfkill/防火墙/转发
    Fix,
    /// 全屏交互界面
    Tui,
}

/// 解析命令行参数。cmd 为 None 表示无子命令。
pub fn parse() -> Cli {
    Cli::parse()
}

#[allow(clippy::needless_return)]
pub fn is_tty() -> bool {
    std::io::IsTerminal::is_terminal(&std::io::stdin())
}

/// 分派子命令; 返回进程退出码。无子命令时按入口策略处理。
pub fn dispatch(cli: Cli) -> i32 {
    match cli.cmd {
        Some(Cmd::Status) => {
            cmd_status();
            0
        }
        Some(Cmd::On { ssid, password }) => cmd_on(ssid.as_deref(), password.as_deref()),
        Some(Cmd::Off) => {
            actions::do_off();
            0
        }
        Some(Cmd::Config {
            ssid,
            password,
            band,
            security,
        }) => cmd_config(
            ssid.as_deref(),
            password.as_deref(),
            band.as_deref(),
            security.as_deref(),
        ),
        Some(Cmd::Devices) => {
            cmd_devices();
            0
        }
        Some(Cmd::Fix) => {
            actions::do_fix();
            0
        }
        Some(Cmd::Tui) => {
            crate::tui::run();
            0
        }
        None => {
            // 无子命令: 交互终端 → TUI; 管道/脚本 → status 文本
            if is_tty() {
                crate::tui::run();
            } else {
                cmd_status();
            }
            0
        }
    }
}

fn cmd_status() {
    let st = status::collect_status();
    print_status(&st);
}

fn print_status(st: &status::Status) {
    use log::c;
    println!("{}", c(log::C_BOLD, "══════════ 热点状态 ══════════"));
    if st.running() {
        println!(
            "  连接:     {}  热点 {}",
            c(log::C_GREEN, "运行中"),
            st.ssid.as_deref().unwrap_or("")
        );
        println!(
            "  接口:     {} @ 10.42.0.1/24",
            st.wlan.as_deref().unwrap_or("-")
        );
        println!(
            "  AP 信息:  type={}, {}",
            st.typ.as_deref().unwrap_or("-"),
            st.channel.as_deref().unwrap_or("-")
        );
    } else {
        println!(
            "  连接:     {}  (可用 hotspotctl on 开启)",
            c(log::C_RED, "未运行")
        );
    }
    let rf_txt = if st.rf_ok() {
        "正常".to_string()
    } else {
        c(log::C_RED, "被屏蔽!")
    };
    println!("  射频:     {}", rf_txt);
    let fwd_txt = if st.ip_forward {
        "已开启".to_string()
    } else {
        c(log::C_RED, "未开启!")
    };
    println!("  IP 转发:  {}", fwd_txt);
    if st.ufw_active {
        println!("  防火墙:   UFW active (deny incoming)");
        for rule in &st.ufw_rules {
            let mark = if rule.found { "✓" } else { "✗" };
            let color = if rule.found { log::C_GREEN } else { log::C_RED };
            println!("    {} {}", c(color, mark), rule.desc);
        }
    } else {
        println!("  防火墙:   {}", c(log::C_RED, "UFW 未运行!"));
    }
    println!("{}", c(log::C_BOLD, "────────── 在线设备 ──────────"));
    if st.stations.is_empty() {
        println!("  (无客户端连接)");
    }
    for s in &st.stations {
        let (ip, host) = st
            .leases
            .get(&s.mac)
            .cloned()
            .unwrap_or(("-".into(), "-".into()));
        let sig = detect::sig_label(s.signal);
        let tx = s.tx.map(|t| format!("↑{t:.0}")).unwrap_or_default();
        println!("  {}  {:<15} {:<12} {:<10} {}", s.mac, ip, host, sig, tx);
    }
    println!(
        "{}",
        c(log::C_DIM, "提示: on / off / config / devices / fix")
    );
}

fn cmd_on(ssid: Option<&str>, password: Option<&str>) -> i32 {
    if actions::do_on(ssid, password) {
        0
    } else {
        1
    }
}

fn cmd_config(
    ssid: Option<&str>,
    password: Option<&str>,
    band_raw: Option<&str>,
    sec_raw: Option<&str>,
) -> i32 {
    let Some(conn) = detect::find_ap_conn() else {
        log::fail("还没有热点连接, 先运行 hotspotctl on");
        return 1;
    };
    let band = match band_raw {
        None => None,
        Some(b) => {
            let b = b.to_lowercase();
            match b.as_str() {
                "auto" => Some(String::new()),
                "2.4" => Some("bg".to_string()),
                "5g" => Some("a".to_string()),
                _ => {
                    log::fail("频段参数: auto / 2.4 / 5g");
                    return 1;
                }
            }
        }
    };
    let sec = match sec_raw {
        None => None,
        Some(s) => {
            let s = s.to_lowercase();
            match s.as_str() {
                "wpa2" => Some("wpa-psk".to_string()),
                "wpa3" => Some("sae".to_string()),
                "open" => Some(String::new()),
                _ => {
                    log::fail("安全算法参数: wpa2 / wpa3 / open");
                    return 1;
                }
            }
        }
    };
    if actions::do_config(&conn, ssid, password, band.as_deref(), sec.as_deref()) {
        0
    } else {
        1
    }
}

fn cmd_devices() {
    let wlan = detect::wlan_iface();
    let leases = detect::get_leases();
    let stations = detect::parse_stations(&wlan);
    if stations.is_empty() {
        println!("(无客户端连接)");
        return;
    }
    println!(
        "{:<20}{:<17}{:<14}{:<10}{:<12}下行",
        "MAC 地址", "IP", "主机名", "信号", "上行"
    );
    for s in &stations {
        print_row(s, &leases);
    }
}

fn print_row(s: &detect::Station, leases: &std::collections::HashMap<String, (String, String)>) {
    let (ip, host) = leases
        .get(&s.mac)
        .cloned()
        .unwrap_or(("-".into(), "-".into()));
    let sig = detect::sig_label(s.signal);
    let tx =
        s.tx.map(|t| format!("{t:.1} M"))
            .unwrap_or_else(|| "-".into());
    let rx =
        s.rx.map(|t| format!("{t:.1} M"))
            .unwrap_or_else(|| "-".into());
    println!(
        "{:<20}{:<17}{:<14}{:<10}{:<12}{}",
        s.mac, ip, host, sig, tx, rx
    );
}
