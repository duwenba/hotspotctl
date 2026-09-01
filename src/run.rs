//! 外部命令执行助手: LANG=C 保证输出可解析, 30s 超时防挂起。

use std::io::Read;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// 执行命令并捕获输出。LANG=C 保证可解析输出; 默认 30s 超时防挂起。
pub fn run(prog: &str, args: &[&str]) -> CommandResult {
    let mut cmd = Command::new(prog);
    cmd.env("LANG", "C");
    cmd.args(args);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return CommandResult {
                status: None,
                stdout: String::new(),
                stderr: format!("无法启动 '{}': {}", prog, e),
                timed_out: false,
            }
        }
    };

    // 读管道: 各起一个线程拉到底, 避免子进程写满管道时死锁。
    let mut out_pipe = child.stdout.take().unwrap();
    let mut err_pipe = child.stderr.take().unwrap();
    let (tx_out, rx_out) = mpsc::channel();
    let (tx_err, rx_err) = mpsc::channel();
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out_pipe.read_to_end(&mut buf);
        let _ = tx_out.send(buf);
    });
    thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = err_pipe.read_to_end(&mut buf);
        let _ = tx_err.send(buf);
    });

    // 等待退出, 带超时。
    let deadline = Instant::now() + Duration::from_secs(30);
    let status: Option<ExitStatus> = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None; // 超时
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };

    let stdout = rx_out
        .recv()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    let stderr = rx_err
        .recv()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();

    CommandResult {
        status,
        stdout,
        stderr,
        timed_out: status.is_none(),
    }
}

/// 命令执行结果。status=None 表示启动失败或超时。
pub struct CommandResult {
    pub status: Option<ExitStatus>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl CommandResult {
    pub fn ok(&self) -> bool {
        self.status.map(|s| s.success()).unwrap_or(false)
    }

    /// 便捷: 失败时返回带 stderr 的错误信息。
    pub fn check(&self, argv: &[&str]) -> Result<(), String> {
        if self.ok() {
            Ok(())
        } else {
            let why = if self.timed_out {
                "超时".to_string()
            } else if let Some(e) = self.status {
                format!("错误码 {}", e.code().unwrap_or(-1))
            } else {
                "启动失败".to_string()
            };
            Err(format!(
                "命令失败: {}\n{} ({})",
                argv.join(" "),
                self.stderr.trim(),
                why,
            ))
        }
    }
}
