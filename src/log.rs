//! 日志层: CLI 模式直接打印, TUI 模式写入事件缓冲由界面渲染。
//! 事件缓冲为全局共享, TUI 从 `events()` 读取渲染日志面板。

use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

pub const C_RESET: &str = "\x1b[0m";
pub const C_BOLD: &str = "\x1b[1m";
pub const C_RED: &str = "\x1b[31m";
pub const C_GREEN: &str = "\x1b[32m";
pub const C_YELLOW: &str = "\x1b[33m";
pub const C_CYAN: &str = "\x1b[36m";
pub const C_DIM: &str = "\x1b[2m";

/// 颜色是否启用(仅当 stdout 是终端); 惰性子初始化。
pub fn color_enabled() -> bool {
    static COLOR: OnceLock<bool> = OnceLock::new();
    *COLOR.get_or_init(|| {
        let stdout = std::io::stdout();
        let locked = stdout.lock();
        locked.is_terminal()
    })
}

impl LogEvent {
    /// TUI 渲染用的 mark 字符。
    pub fn mark(&self) -> &'static str {
        self.level.mark()
    }
}

impl Level {
    pub fn mark(&self) -> &'static str {
        match self {
            Level::Ok => "✓",
            Level::Fail => "✗",
            Level::Warn => "⚠",
            Level::Info => "ℹ",
        }
    }
    pub fn ansi(&self) -> &'static str {
        match self {
            Level::Ok => C_GREEN,
            Level::Fail => C_RED,
            Level::Warn => C_YELLOW,
            Level::Info => C_CYAN,
        }
    }
    /// TUI(ratatui)样式色名。
    pub fn tui_color(&self) -> ratatui::style::Color {
        match self {
            Level::Ok => ratatui::style::Color::Green,
            Level::Fail => ratatui::style::Color::Red,
            Level::Warn => ratatui::style::Color::Yellow,
            Level::Info => ratatui::style::Color::Cyan,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    Fail,
    Warn,
    Info,
}

#[derive(Clone, Debug)]
pub struct LogEvent {
    pub level: Level,
    pub msg: String,
}

/// TUI 模式下只入缓冲, 不打印; CLI 模式直接打印。
static TUI_MODE: AtomicBool = AtomicBool::new(false);

pub fn set_tui_mode(on: bool) {
    TUI_MODE.store(on, Ordering::SeqCst);
}

pub fn is_tui_mode() -> bool {
    TUI_MODE.load(Ordering::SeqCst)
}

fn events_store() -> &'static Mutex<Vec<LogEvent>> {
    static EV: OnceLock<Mutex<Vec<LogEvent>>> = OnceLock::new();
    EV.get_or_init(|| Mutex::new(Vec::new()))
}

/// 追加一条事件。
pub fn emit(level: Level, msg: &str) {
    if let Ok(mut ev) = events_store().lock() {
        ev.push(LogEvent {
            level,
            msg: msg.to_string(),
        });
    }
    if !TUI_MODE.load(Ordering::SeqCst) {
        let color = color_enabled();
        let prefix = if color {
            format!("  {} {} {}", level.ansi(), level.mark(), msg)
        } else {
            format!("  {} {}", level.mark(), msg)
        };
        println!("{prefix}");
    }
}

/// 取事件缓冲克隆(TUI 渲染用), 保留最近 `n` 条。
pub fn events(n: usize) -> Vec<LogEvent> {
    let store = events_store().lock().unwrap();
    let len = store.len();
    let start = len.saturating_sub(n);
    store[start..].to_vec()
}

/// 清空事件缓冲(可选的)。
#[allow(dead_code)]
pub fn clear() {
    if let Ok(mut ev) = events_store().lock() {
        ev.clear();
    }
}

pub fn ok(msg: &str) {
    emit(Level::Ok, msg);
}
pub fn fail(msg: &str) {
    emit(Level::Fail, msg);
}
pub fn warn(msg: &str) {
    emit(Level::Warn, msg);
}
pub fn info(msg: &str) {
    emit(Level::Info, msg);
}

/// 带颜色的文本包装(非终端时返回原文)。
pub fn c(code: &str, text: &str) -> String {
    if color_enabled() {
        format!("{}{}{}", code, text, C_RESET)
    } else {
        text.to_string()
    }
}
