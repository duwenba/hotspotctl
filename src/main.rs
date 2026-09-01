// hotspotctl — Linux 便携热点管理器 (NetworkManager + UFW)
//
// 子命令: status / on / off / config / devices / fix / tui
// 需要 root 权限。检测到非 root 时自动通过 run0 提权, 桌面会弹出 polkit 密码窗口。

mod actions;
mod cli;
mod detect;
mod log;
mod run;
mod status;
mod tui;

use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::process::Command;

/// 读取当前 euid(读取 /proc/self/status 的 Uid: 首字段, 无需 libc 依赖)。
fn geteuid() -> u32 {
    let Ok(contents) = std::fs::read_to_string("/proc/self/status") else {
        return u32::MAX;
    };
    for line in contents.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            return rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(u32::MAX);
        }
    }
    u32::MAX
}

/// 非 root 时通过 run0 以 root 重新执行自身(替换当前进程)。
fn ensure_root() {
    if geteuid() == 0 {
        return;
    }
    println!(
        "{}需要 root 权限, 通过 run0 提权 (桌面将弹出密码窗口)...",
        crate::log::C_YELLOW
    );
    let exe = std::env::current_exe().expect("获取当前可执行路径失败");
    let mut args: Vec<OsString> = vec![exe.into_os_string()];
    args.extend(std::env::args_os().skip(1));
    let err = Command::new("run0").arg("--background=").args(&args).exec();
    panic!("run0 re-exec 失败: {err}");
}

fn main() {
    ensure_root();
    let cli = cli::parse();
    let code = cli::dispatch(cli);
    std::process::exit(code);
}
