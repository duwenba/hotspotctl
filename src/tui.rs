//! TUI: ratatui 全屏界面。三面板主屏 + 模态配置表单 + 热键。
//! 后台线程采集状态(2s)与执行动作, 不阻塞界面事件循环。

use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Padding, Paragraph,
};
use ratatui::{Frame, Terminal};

use crate::actions;
use crate::detect;
use crate::log;
use crate::status::{self, Status};

const BAND_OPTS: [(&str, &str); 3] = [("自动", ""), ("2.4GHz", "bg"), ("5GHz", "a")];

/// 主题配色: 全部用前景色/边框色, 不依赖终端主题背景, run0 提权后也可读。
const BORDER: Color = Color::DarkGray;
const TITLE: Color = Color::Cyan;
const ACCENT: Color = Color::Cyan;
const OK: Color = Color::Green;
const WARN: Color = Color::Yellow;
const BAD: Color = Color::Red;
const DIM: Color = Color::DarkGray;
const SEC_OPTS: [(&str, &str); 3] = [
    ("WPA2 (wpa-psk)", "wpa-psk"),
    ("WPA3 (sae)", "sae"),
    ("开放 (无密码)", ""),
];

enum Screen {
    Main,
    Config(ConfigState),
}

/// 配置表单状态。
struct ConfigState {
    conn: String,
    init_ssid: String,
    init_band: String,
    init_sec: String,
    ssid_buf: String,
    pwd_buf: String,
    band_idx: usize,
    sec_idx: usize,
    focus: usize,
}

pub struct App {
    st: Arc<Mutex<Option<Status>>>,
    show_pwd: bool,
    screen: Screen,
    exit_requested: bool,
    device_cursor: usize, // 设备列表光标(指向 st.stations 索引)
    device_detail: bool,  // 是否处于"选中设备详情"视图
}

/// 设备列表显示名: 租约主机名优先, 空则兑底 MAC。
fn device_name(st: &Status, s: &detect::Station) -> String {
    st.leases
        .get(&s.mac)
        .map(|(_, host)| {
            if host.is_empty() {
                s.mac.clone()
            } else {
                host.clone()
            }
        })
        .unwrap_or_else(|| s.mac.clone())
}

fn band_index(v: &str) -> usize {
    BAND_OPTS.iter().position(|(_, val)| *val == v).unwrap_or(0)
}
fn sec_index(v: &str) -> usize {
    SEC_OPTS.iter().position(|(_, val)| *val == v).unwrap_or(2)
}

impl App {
    fn new() -> Self {
        let st = Arc::new(Mutex::new(None));
        App {
            st,
            show_pwd: false,
            screen: Screen::Main,
            exit_requested: false,
            device_cursor: 0,
            device_detail: false,
        }
    }

    /// 后台采集线程: 每 2s 更新状态。
    fn start_refresh(&self) {
        let st = self.st.clone();
        thread::spawn(move || loop {
            *st.lock().unwrap() = Some(status::collect_status());
            thread::sleep(Duration::from_secs(2));
        });
    }

    /// 立即刷新一次(动作完成后调用)。
    fn refresh_now(&self) {
        *self.st.lock().unwrap() = Some(status::collect_status());
    }

    /// 后台执行动作; 结束后刷新状态。
    fn spawn_action(&self, f: impl Fn() + Send + 'static) {
        let st = self.st.clone();
        thread::spawn(move || {
            f();
            *st.lock().unwrap() = Some(status::collect_status());
        });
    }

    /// 主循环事件处理。返回是否退出。
    fn handle(&mut self, ev: &Event) -> bool {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Press => return false,
            Event::Key(k) => match &mut self.screen {
                Screen::Main => self.handle_main(*k),
                Screen::Config(_) => {
                    if let Some(outcome) = self.handle_config(*k) {
                        match outcome {
                            ConfigOutcome::Commit(vals) => {
                                if let Screen::Config(c) = &self.screen {
                                    let conn = c.conn.clone();
                                    self.spawn_action(move || {
                                        actions::do_config(
                                            &conn,
                                            vals.ssid.as_deref(),
                                            vals.pwd.as_deref(),
                                            vals.band.as_deref(),
                                            vals.sec.as_deref(),
                                        );
                                    });
                                }
                                self.screen = Screen::Main;
                                self.refresh_now();
                            }
                            ConfigOutcome::Cancel => {
                                log::info("已取消配置");
                                self.screen = Screen::Main;
                                self.refresh_now();
                            }
                            ConfigOutcome::Stay => {}
                        }
                    }
                }
            },
            _ => {}
        }
        self.exit_requested
    }

    fn handle_main(&mut self, k: KeyEvent) {
        let n_stations = self
            .st
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| s.stations.len())
            .unwrap_or(0);
        match k.code {
            KeyCode::Char('q') => self.exit_requested = true,
            KeyCode::Char('o') => self.spawn_action(move || {
                actions::do_on(None, None);
            }),
            KeyCode::Char('f') => self.spawn_action(actions::do_off),
            KeyCode::Char('r') => self.spawn_action(actions::do_fix),
            KeyCode::Char('s') => self.refresh_now(),
            KeyCode::Char('p') => {
                self.show_pwd = !self.show_pwd;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.device_cursor = self.device_cursor.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.device_cursor = self
                    .device_cursor
                    .saturating_add(1)
                    .min(n_stations.saturating_sub(1));
            }
            KeyCode::Enter => {
                if self.device_detail {
                    self.device_detail = false;
                } else if n_stations > 0 {
                    self.device_detail = true;
                }
            }
            KeyCode::Esc => {
                self.device_detail = false;
            }
            KeyCode::Char('c') => {
                if let Some(conn) = detect::find_ap_conn() {
                    let init_band = detect::get_band(&conn);
                    let init_sec = detect::get_key_mgmt(&conn);
                    let band_idx = band_index(&init_band);
                    let sec_idx = sec_index(&init_sec);
                    self.screen = Screen::Config(ConfigState {
                        conn: conn.clone(),
                        init_ssid: detect::get_ssid(&conn),
                        init_band,
                        init_sec,
                        ssid_buf: String::new(),
                        pwd_buf: String::new(),
                        band_idx,
                        sec_idx,
                        focus: 0,
                    })
                } else {
                    log::warn("还没有热点连接, 请先开启热点 (按 o)");
                    self.refresh_now();
                }
            }
            _ => {}
        }
    }

    fn handle_config(&mut self, k: KeyEvent) -> Option<ConfigOutcome> {
        let Screen::Config(c) = &mut self.screen else {
            return None;
        };
        match k.code {
            KeyCode::Esc => return Some(ConfigOutcome::Cancel),
            KeyCode::Enter => {
                let ssid =
                    (c.ssid_buf.trim() != c.init_ssid).then(|| c.ssid_buf.trim().to_string());
                let pwd = (!c.pwd_buf.is_empty()).then(|| c.pwd_buf.clone());
                let band_val = BAND_OPTS[c.band_idx].1;
                let sec_val = SEC_OPTS[c.sec_idx].1;
                let band = (band_val != c.init_band).then(|| band_val.to_string());
                let sec = (sec_val != c.init_sec).then(|| sec_val.to_string());
                return Some(ConfigOutcome::Commit(ConfigVals {
                    ssid,
                    pwd,
                    band,
                    sec,
                }));
            }
            KeyCode::Tab | KeyCode::Down => {
                c.focus = (c.focus + 1) % 4;
            }
            KeyCode::Up => {
                c.focus = if c.focus == 0 { 3 } else { c.focus - 1 };
            }
            KeyCode::Right => {
                if c.focus == 2 {
                    c.band_idx = (c.band_idx + 1) % BAND_OPTS.len();
                } else if c.focus == 3 {
                    c.sec_idx = (c.sec_idx + 1) % SEC_OPTS.len();
                }
            }
            KeyCode::Left => {
                if c.focus == 2 {
                    c.band_idx = (c.band_idx + BAND_OPTS.len() - 1) % BAND_OPTS.len();
                } else if c.focus == 3 {
                    c.sec_idx = (c.sec_idx + SEC_OPTS.len() - 1) % SEC_OPTS.len();
                }
            }
            KeyCode::Backspace => {
                if c.focus == 0 {
                    c.ssid_buf.pop();
                } else if c.focus == 1 {
                    c.pwd_buf.pop();
                }
            }
            KeyCode::Char(ch) => {
                if ch == 'c' && k.modifiers.contains(KeyModifiers::CONTROL) {
                    return None;
                }
                if c.focus == 0 {
                    c.ssid_buf.push(ch);
                } else if c.focus == 1 {
                    c.pwd_buf.push(ch);
                }
            }
            _ => {}
        }
        Some(ConfigOutcome::Stay)
    }
}

struct ConfigVals {
    ssid: Option<String>,
    pwd: Option<String>,
    band: Option<String>,
    sec: Option<String>,
}

enum ConfigOutcome {
    Commit(ConfigVals),
    Cancel,
    Stay,
}

/// 渲染入口。
impl App {
    fn render(&self, frame: &mut Frame) {
        let area = frame.area();
        if area.width == 0 || area.height == 0 {
            return; // 0×0 终端(如 script 无尺寸的管道)下跳过渲染, 避免越界。
        }
        // 先铺一层黑色背景。run0 提权后 root 环境的终端备用屏默认背景色可能异常(变红/变亮),
        // 显式设置底色保证在任何用户/主题下都一致。
        frame.render_widget(
            Paragraph::new("").style(Style::default().bg(ratatui::style::Color::Reset)),
            area,
        );
        match &self.screen {
            Screen::Main => self.render_main(frame),
            Screen::Config(_) => {
                self.render_main(frame);
                self.render_config(frame);
            }
        }
    }

    fn render_main(&self, frame: &mut Frame) {
        let st = self.st.lock().unwrap().clone();
        let area = frame.area();

        let mid = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(1)])
            .split(area);
        self.render_body(frame, mid[0], &st);
        self.render_footer(frame, mid[1], &st);
    }

    fn render_body(&self, frame: &mut Frame, area: ratatui::layout::Rect, st: &Option<Status>) {
        // 上下留 1 行空白, 左右留 1 列空白, 让各面板之间留白、不贴边。
        let outer = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(0),
                Constraint::Min(10),
                Constraint::Length(0),
            ])
            .split(area);
        let inner = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0), Constraint::Length(10)])
            .split(outer[1]);

        let inner_pad = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(10),
                Constraint::Length(1),
            ])
            .split(inner[0]);

        let panels = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(40),
                Constraint::Min(0),
                Constraint::Percentage(60),
            ])
            .split(inner_pad[1]);

        // 左栏: 设备列表
        self.render_devices(frame, panels[0], st);
        // 右栏: 选中设备详情(Enter 进入) 或 状态总览
        let show_detail = self.device_detail && st.as_ref().is_some_and(|s| !s.stations.is_empty());
        if show_detail {
            self.render_detail(frame, panels[2], st.as_ref().unwrap());
        } else {
            self.render_status(frame, panels[2], st);
        }
        self.render_log(frame, inner[1]);
    }

    fn render_status(&self, frame: &mut Frame, area: ratatui::layout::Rect, st: &Option<Status>) {
        let Some(st) = st else {
            let lines = vec![Line::from(vec![Span::styled(
                " 无法获取状态",
                Style::default().fg(BAD),
            )])];
            frame.render_widget(Paragraph::new(lines).block(panel("状态")), area);
            return;
        };

        let mut lines: Vec<Line<'static>> = Vec::new();

        // —— 连接概况 ——
        if st.running() {
            lines.push(Line::from(vec![Span::styled(
                format!(" ● {}", st.ssid.as_deref().unwrap_or("运行中")),
                Style::default().fg(OK).add_modifier(Modifier::BOLD),
            )]));
            if let Some(pwd) = &st.password {
                let shown = if self.show_pwd {
                    pwd.clone()
                } else {
                    "*".repeat(pwd.chars().count())
                };
                lines.push(kv("密码", &shown, None));
            } else {
                lines.push(Line::from(vec![
                    Span::styled(pad("密码", 8), Style::default()),
                    Span::styled("未设置(开放网络)", Style::default()),
                ]));
            }
            lines.push(kv(
                "接口",
                &format!("{} @ 10.42.0.1/24", st.wlan.as_deref().unwrap_or("-")),
                None,
            ));
            let mut chan = "-".to_string();
            if let Some(ch) = &st.channel {
                if let Some(d) = ch.chars().next().filter(|c| c.is_ascii_digit()) {
                    chan = d.to_string();
                }
            }
            lines.push(kv("信道", &chan, None));
        } else {
            lines.push(Line::from(Span::styled(
                " ○ 未运行 ",
                Style::default().fg(BAD).add_modifier(Modifier::BOLD),
            )));
        }

        lines.push(Line::from(Span::styled(
            " ─────────────────────────────",
            Style::default().fg(BORDER),
        )));

        // —— 检查项 ——
        lines.push(badge("射频", st.rf_ok(), "正常", "被屏蔽!"));
        lines.push(badge("转发", st.ip_forward, "已开启", "未开启!"));
        if st.ufw_active {
            lines.push(badge("防火墙", true, "UFW active (deny incoming)", ""));
            let mut marks = String::new();
            for rule in &st.ufw_rules {
                marks.push_str(if rule.found { "✓ " } else { "✗ " });
                marks.push_str(&rule.desc);
                marks.push_str("  ");
            }
            lines.push(Line::from(Span::styled(
                format!("          {marks}"),
                Style::default(),
            )));
        } else {
            lines.push(badge("防火墙", false, "", "UFW 未运行!"));
        }

        // 边框顶标题“状态”, 底标题放密码显隐提示(仅在设置了密码时)。
        let tip = if self.show_pwd {
            "按 p 隐藏"
        } else {
            "按 p 显示"
        };
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(BORDER))
            .padding(Padding::horizontal(1))
            .title(Span::styled(
                " 状态 ",
                Style::default().fg(TITLE).add_modifier(Modifier::BOLD),
            ));
        if st.password.is_some() {
            block = block.title_bottom(Span::styled(tip, Style::default().fg(DIM)));
        }
        frame.render_widget(Paragraph::new(lines).block(block), area);
    }

    fn render_devices(&self, frame: &mut Frame, area: ratatui::layout::Rect, st: &Option<Status>) {
        let tip = if self.device_detail {
            "↑↓ 选择 · Enter/Esc 返回"
        } else {
            "↑↓ 选择 · Enter 查看详情"
        };

        let mut items: Vec<ListItem> = Vec::new();
        if let Some(s) = st {
            for stn in &s.stations {
                let name = device_name(s, stn);
                // 每行: 设备名 + 信号等级徽标。
                let sc = sig_color(stn.signal);
                let sig = if stn.signal.is_some() {
                    detect::sig_label(stn.signal)
                } else {
                    "-"
                };
                let sig_num = stn.signal.map(|v| format!("({v} dBm)")).unwrap_or_default();
                items.push(ListItem::new(Line::from(vec![
                    Span::styled(format!(" {name:<16}"), Style::default().fg(Color::White)),
                    Span::styled(
                        format!("{sig:<6}"),
                        Style::default().fg(sc).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(sig_num, Style::default().fg(DIM)),
                ])));
            }
        }
        if items.is_empty() {
            items.push(ListItem::new(Line::from(vec![Span::styled(
                " (无客户端连接)",
                Style::default().fg(DIM),
            )])));
        }
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(BORDER))
            .padding(Padding::horizontal(1))
            .title(Span::styled(
                " 设备 ",
                Style::default().fg(TITLE).add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Span::styled(tip, Style::default().fg(DIM)));
        let list = List::new(items)
            .block(block)
            // 选中行只加粗+变亮, 不 REVERSED/加背景, 提权后也不会出现异常底色。
            .highlight_style(
                Style::default()
                    .bg(Color::Black)
                    .fg(Color::LightGreen)
                    .add_modifier(Modifier::BOLD),
            );
        let mut state = ListState::default();
        let n_devs = st.as_ref().map(|s| s.stations.len()).unwrap_or(0);
        if n_devs > 0 {
            let cursor = self.device_cursor.min(n_devs.saturating_sub(1));
            state.select(Some(cursor));
        } else {
            state.select(None);
        }
        frame.render_stateful_widget(list, area, &mut state);
    }

    /// 选中设备的详细信息(btop 进程式详情面板)。
    fn render_detail(&self, frame: &mut Frame, area: ratatui::layout::Rect, st: &Status) {
        let idx = self.device_cursor.min(st.stations.len().saturating_sub(1));
        let s = &st.stations[idx];
        let name = device_name(st, s);
        let (ip, host) = st
            .leases
            .get(&s.mac)
            .cloned()
            .unwrap_or(("-".into(), "-".into()));
        let host = if host.is_empty() || host == "-" {
            "未知".to_string()
        } else {
            host
        };
        let sig = detect::sig_label(s.signal);
        let sig_num = s.signal.map(|v| format!(" ({v} dBm)")).unwrap_or_default();
        let tx =
            s.tx.map(|t| format!("{t:.1} M"))
                .unwrap_or_else(|| "-".into());
        let rx =
            s.rx.map(|t| format!("{t:.1} M"))
                .unwrap_or_else(|| "-".into());

        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(Line::from(Span::styled(
            format!("◆ {name}"),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));
        lines.push(kv("主机名", &host, None));
        lines.push(kv("MAC", &s.mac, None));
        lines.push(kv("IP", &ip, None));
        lines.push(Line::from(vec![
            Span::styled(
                pad("信号", 8),
                Style::default().fg(DIM).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                sig,
                Style::default()
                    .fg(sig_color(s.signal))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(sig_num, Style::default().fg(DIM)),
        ]));
        lines.push(kv("上行", &tx, Some(OK)));
        lines.push(kv("下行", &rx, Some(OK)));

        let p = Paragraph::new(lines).block(panel("设备详情"));
        frame.render_widget(p, area);
    }

    fn render_log(&self, frame: &mut Frame, area: ratatui::layout::Rect) {
        let mut lines: Vec<Line<'static>> = Vec::new();
        for ev in log::events(14) {
            lines.push(Line::from(vec![
                Span::styled(ev.mark(), Style::default().fg(ev.level.tui_color())),
                Span::raw(format!(" {}", ev.msg)),
            ]));
        }
        if lines.is_empty() {
            lines.push(Line::from(Span::styled(
                " 暂无日志",
                Style::default().fg(DIM),
            )));
        }
        frame.render_widget(Paragraph::new(lines).block(panel("日志")), area);
    }

    /// 屏幕最底部状态栏(jj 风格): 全用前景色高亮, 不用背景色块/标题。
    fn render_footer(&self, frame: &mut Frame, area: ratatui::layout::Rect, st: &Option<Status>) {
        let hint: [(&str, &str); 7] = [
            ("o", "开启"),
            ("f", "关闭"),
            ("r", "修复"),
            ("c", "配置"),
            ("s", "刷新"),
            ("p", "密码"),
            ("q", "退出"),
        ];
        let mut spans: Vec<Span<'static>> = Vec::new();

        // 左侧: 连接状态词, 前景色高亮(蓝/青底粉转通用)。
        let (state_word, state_color) = match st {
            Some(s) if s.running() => ("● 运行中", OK),
            Some(_) => ("○ 未运行", BAD),
            None => ("○ 检测中", WARN),
        };
        spans.push(Span::styled(
            format!("  {state_word}  "),
            Style::default().fg(state_color),
        ));

        // 中间键位提示: 键(青色前景) + 说明(灰色)。
        for (i, (key, desc)) in hint.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" · ", Style::default().fg(BORDER)));
            }
            spans.push(Span::styled(format!("{key}"), Style::default().fg(ACCENT)));
            spans.push(Span::styled(
                format!(" {desc}"),
                Style::default().fg(Color::White),
            ));
        }

        // 右侧环境信息: wlan @ 接口, 前景色高亮右对齐。
        let env = match st {
            Some(s) => format!("{} @ 10.42.0.1/24", s.wlan.as_deref().unwrap_or("-")),
            None => "-".to_string(),
        };
        let env_text = format!("  {env}");
        let env_disp: usize = env_text
            .chars()
            .map(|c| if c.len_utf8() > 1 { 2 } else { 1 })
            .sum();
        let left: usize = spans
            .iter()
            .map(|s| {
                s.content
                    .chars()
                    .map(|c| if c.len_utf8() > 1 { 2 } else { 1 })
                    .sum::<usize>()
            })
            .sum();
        let fill = (area.width as usize).saturating_sub(left + env_disp);
        spans.push(Span::styled(" ".repeat(fill), Style::default()));
        spans.push(Span::styled(
            env_text,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::Black)),
            area,
        );
    }
}

/// 按显示宽度(全角=2, 半角=1)填充到等宽, 用于中英文混排对齐。
fn pad(s: &str, w: usize) -> String {
    let cur: usize = s
        .chars()
        .map(|c| if c.len_utf8() > 1 { 2 } else { 1 })
        .sum();
    format!("{s}{}", " ".repeat(w.saturating_sub(cur)))
}

/// 统一面板: 圆角边框 + 淡色标题 + 左右内边距, 风格一致。
fn panel(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER))
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(TITLE).add_modifier(Modifier::BOLD),
        ))
}

/// label 对齐 + 值的行。
fn kv(label: &str, val: &str, val_color: Option<Color>) -> Line<'static> {
    let mut spans = vec![Span::styled(pad(label, 8), Style::default())];
    let s = val.to_string();
    spans.push(match val_color {
        Some(c) => Span::styled(s, Style::default().fg(c)),
        None => Span::raw(s),
    });
    Line::from(spans)
}

/// ✓/✗ 状态徽章行。
fn badge(label: &str, ok: bool, ok_text: &str, bad_text: &str) -> Line<'static> {
    let (mark, color, text) = if ok {
        ("✓", OK, ok_text)
    } else {
        ("✗", BAD, bad_text)
    };
    Line::from(vec![
        Span::styled(
            format!(" {mark} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(pad(label, 8), Style::default()),
        Span::styled(text.to_string(), Style::default().fg(color)),
    ])
}

/// 信号强度颜色: 强=绿 / 中=黄 / 弱=红 / 未知=灰。
fn sig_color(signal: Option<i32>) -> Color {
    match signal {
        Some(v) if v >= -50 => OK,
        Some(v) if v >= -75 => WARN,
        Some(_) => BAD,
        None => DIM,
    }
}

/// 配置表单渲染(居中模态)。
impl App {
    fn render_config(&self, frame: &mut Frame) {
        let Screen::Config(c) = &self.screen else {
            return;
        };
        let area = frame.area();
        let w = 66u16.min(area.width.saturating_sub(2).max(20));
        let h = 14u16;
        let x = area.width.saturating_sub(w) / 2;
        let y = area.height.saturating_sub(h) / 2;
        let modal = ratatui::layout::Rect::new(x, y, w, h);

        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(Line::from(vec![
            Span::styled(
                "连接 ",
                Style::default().fg(DIM).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                c.conn.clone(),
                Style::default().fg(TITLE).add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(""));

        for (i, label, shown) in [
            (0, "网络名称 (SSID)", c.ssid_buf.clone()),
            (
                1,
                "密码 (≥8位, 留空则不修改)",
                "*".repeat(c.pwd_buf.chars().count()),
            ),
            (
                2,
                "AP 频段",
                format!("{} (按 ←/→ 切换)", BAND_OPTS[c.band_idx].0),
            ),
            (
                3,
                "安全算法",
                format!("{} (按 ←/→ 切换)", SEC_OPTS[c.sec_idx].0),
            ),
        ] {
            let focused = c.focus == i;
            // 选中行用「前景色 + 下划线」标识, 不用 REVERSED/背景色,
            // 避免在提权后触底成错误背景色或因主题变化看不清。
            let style = if focused {
                Style::default()
                    .fg(WARN)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default().fg(Color::White)
            };
            let text = format!("{}{}", pad(label, 30), shown);
            lines.push(Line::from(Span::styled(text, style)));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            " Enter 保存   Esc 取消   Tab/↑↓ 切换输入  ←/→ 切换选项",
            Style::default().fg(DIM),
        )));

        // 先用 Clear 清掉弹窗区域, 避免主屏文字穿透到弹窗内部。
        frame.render_widget(Clear, modal);
        // 弹窗透明(不填背景色), 紧随终端主题背景, 提权后也不会出现异常底色。
        let p = Paragraph::new(lines).block(panel("配置"));
        frame.render_widget(p, modal);
    }
}

/// 启动 TUI 主循环。
pub fn run() {
    let _ = io::stdout().flush();
    enable_raw_mode().expect("enable_raw_mode 失败");
    let mut terminal = match Terminal::new(CrosstermBackend::new(io::stdout())) {
        Ok(t) => t,
        Err(e) => {
            let _ = disable_raw_mode();
            eprintln!("初始化终端失败: {e}");
            return;
        }
    };
    let _ = execute!(terminal.backend_mut(), EnterAlternateScreen);

    let mut app = App::new();
    app.start_refresh();
    {
        let st = app.st.clone();
        *st.lock().unwrap() = Some(status::collect_status());
    }
    log::set_tui_mode(true);

    'outer: loop {
        let _ = terminal.draw(|frame| app.render(frame));
        if crossterm::event::poll(Duration::from_millis(100)).unwrap_or(false) {
            match event::read() {
                Ok(ev) => {
                    if app.handle(&ev) {
                        break 'outer;
                    }
                }
                Err(_) => break 'outer,
            }
        }
    }

    let _ = execute!(terminal.backend_mut(), LeaveAlternateScreen);
    let _ = disable_raw_mode();
    let _ = io::stdout().flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn sample_status() -> Status {
        use std::collections::HashMap;
        let mut leases = HashMap::new();
        leases.insert(
            "aa:bb:cc:dd:ee:ff".to_string(),
            ("192.168.1.5".to_string(), "MEIZU-20".to_string()),
        );
        Status {
            wlan: Some("wlan0".to_string()),
            state: Some("connected".to_string()),
            conn: Some("Hotspot".to_string()),
            ssid: Some("Recluse".to_string()),
            typ: Some("ap".to_string()),
            channel: Some("6".to_string()),
            password: Some("secretpw".to_string()),
            soft_block: Some(false),
            hard_block: Some(false),
            ip_forward: true,
            ufw_active: true,
            ufw_rules: vec![
                crate::status::UfwRule {
                    desc: "DHCP 放行".into(),
                    found: true,
                },
                crate::status::UfwRule {
                    desc: "子网放行".into(),
                    found: true,
                },
                crate::status::UfwRule {
                    desc: "转发放行".into(),
                    found: true,
                },
            ],
            leases,
            stations: vec![
                crate::detect::Station {
                    mac: "aa:bb:cc:dd:ee:ff".into(),
                    signal: Some(-45),
                    tx: Some(433.3),
                    rx: Some(866.7),
                },
                crate::detect::Station {
                    mac: "11:22:33:44:55:66".into(),
                    signal: Some(-70),
                    tx: None,
                    rx: None,
                },
            ],
        }
    }

    #[allow(dead_code)]
    fn dump(terminal: &Terminal<TestBackend>) {
        let buf = terminal.backend().buffer().clone();
        let area = buf.area();
        for y in 0..area.height {
            let mut row = String::new();
            for x in 0..area.width {
                row.push_str(&buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(""));
            }
            println!("{:>3} {}", y, row);
        }
    }

    #[test]
    fn render_main_snapshot() {
        let backend = TestBackend::new(190, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let app = App::new();
        *app.st.lock().unwrap() = Some(sample_status());
        terminal.draw(|f| app.render(f)).unwrap();
        dump(&terminal);
    }

    #[test]
    fn render_config_snapshot() {
        let backend = TestBackend::new(190, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = App::new();
        *app.st.lock().unwrap() = Some(sample_status());
        app.screen = Screen::Config(ConfigState {
            conn: "Hotspot".into(),
            init_ssid: "Recluse".into(),
            init_band: "".into(),
            init_sec: "wpa-psk".into(),
            ssid_buf: "Recluse".into(),
            pwd_buf: "secretpw".into(),
            band_idx: 0,
            sec_idx: 0,
            focus: 0,
        });
        terminal.draw(|f| app.render(f)).unwrap();
        dump(&terminal);
    }
}
