//! Terminal telemetry dashboard using ratatui.
//!
//! Provides a real-time TUI dashboard showing:
//! - Agent status and current session
//! - Connected peripheral nodes and their health
//! - Live tool call log
//! - System metrics (CPU, memory, uptime)
//! - Scheduler task status
//!
//! Launch with: `oh-ben-claw dashboard`

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ── Dashboard State ───────────────────────────────────────────────────────────

/// A single entry in the live event log.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardEvent {
    pub timestamp: u64,
    pub level: EventLevel,
    pub source: String,
    pub message: String,
}

/// Severity level for dashboard events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EventLevel {
    Info,
    Success,
    Warning,
    Error,
}

impl EventLevel {
    pub fn label(&self) -> &str {
        match self {
            EventLevel::Info => "INFO",
            EventLevel::Success => "OK  ",
            EventLevel::Warning => "WARN",
            EventLevel::Error => "ERR ",
        }
    }
}

/// Status of a peripheral node for dashboard display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeStatus {
    pub name: String,
    pub board_type: String,
    pub status: String,
    pub tool_count: usize,
    pub last_seen: u64,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

/// Snapshot of agent metrics for dashboard display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentMetrics {
    pub is_running: bool,
    pub is_busy: bool,
    pub current_session: String,
    pub total_turns: u64,
    pub total_tool_calls: u64,
    pub uptime_secs: u64,
    pub provider: String,
    pub model: String,
}

/// Snapshot of system metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMetrics {
    pub cpu_percent: f32,
    pub memory_used_mb: u64,
    pub memory_total_mb: u64,
    pub disk_used_gb: f32,
    pub disk_total_gb: f32,
}

/// Shared dashboard state, updated by background tasks.
pub struct DashboardState {
    pub agent: Arc<Mutex<AgentMetrics>>,
    pub nodes: Arc<Mutex<Vec<NodeStatus>>>,
    pub events: Arc<Mutex<VecDeque<DashboardEvent>>>,
    pub system: Arc<Mutex<SystemMetrics>>,
    pub max_events: usize,
    pub started_at: Instant,
}

impl DashboardState {
    pub fn new() -> Self {
        Self {
            agent: Arc::new(Mutex::new(AgentMetrics {
                is_running: false,
                is_busy: false,
                current_session: "default".to_string(),
                total_turns: 0,
                total_tool_calls: 0,
                uptime_secs: 0,
                provider: "openai".to_string(),
                model: "gpt-5".to_string(),
            })),
            nodes: Arc::new(Mutex::new(Vec::new())),
            events: Arc::new(Mutex::new(VecDeque::new())),
            system: Arc::new(Mutex::new(SystemMetrics {
                cpu_percent: 0.0,
                memory_used_mb: 0,
                memory_total_mb: 0,
                disk_used_gb: 0.0,
                disk_total_gb: 0.0,
            })),
            max_events: 500,
            started_at: Instant::now(),
        }
    }

    /// Push a new event to the log, evicting old ones if at capacity.
    pub fn push_event(&self, level: EventLevel, source: &str, message: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let event = DashboardEvent {
            timestamp: now,
            level,
            source: source.to_string(),
            message: message.to_string(),
        };

        let mut events = self.events.lock().unwrap();
        if events.len() >= self.max_events {
            events.pop_front();
        }
        events.push_back(event);
    }

    /// Update agent metrics.
    pub fn update_agent(&self, metrics: AgentMetrics) {
        *self.agent.lock().unwrap() = metrics;
    }

    /// Update node status.
    pub fn update_node(&self, node: NodeStatus) {
        let mut nodes = self.nodes.lock().unwrap();
        if let Some(existing) = nodes.iter_mut().find(|n| n.name == node.name) {
            *existing = node;
        } else {
            nodes.push(node);
        }
    }

    /// Remove a node from the status list.
    pub fn remove_node(&self, name: &str) {
        let mut nodes = self.nodes.lock().unwrap();
        nodes.retain(|n| n.name != name);
    }

    /// Update system metrics by reading /proc/meminfo and /proc/stat.
    pub fn refresh_system_metrics(&self) {
        let metrics = read_system_metrics();
        *self.system.lock().unwrap() = metrics;
    }

    pub fn uptime(&self) -> Duration {
        self.started_at.elapsed()
    }
}

impl Default for DashboardState {
    fn default() -> Self {
        Self::new()
    }
}

// ── System Metrics Reader ─────────────────────────────────────────────────────

fn read_system_metrics() -> SystemMetrics {
    let (memory_used_mb, memory_total_mb) = read_memory_info();
    let cpu_percent = read_cpu_percent();
    let (disk_used_gb, disk_total_gb) = read_disk_info();

    SystemMetrics {
        cpu_percent,
        memory_used_mb,
        memory_total_mb,
        disk_used_gb,
        disk_total_gb,
    }
}

fn read_memory_info() -> (u64, u64) {
    let content = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mut total = 0u64;
    let mut available = 0u64;

    for line in content.lines() {
        if line.starts_with("MemTotal:") {
            total = line
                .split_whitespace()
                .nth(1)
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
        } else if line.starts_with("MemAvailable:") {
            available = line
                .split_whitespace()
                .nth(1)
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0);
        }
    }

    let total_mb = total / 1024;
    let used_mb = (total.saturating_sub(available)) / 1024;
    (used_mb, total_mb)
}

fn read_cpu_percent() -> f32 {
    // Read /proc/stat twice with a short delay to compute CPU usage
    let read_stat = || -> Option<(u64, u64)> {
        let content = std::fs::read_to_string("/proc/stat").ok()?;
        let line = content.lines().next()?;
        let values: Vec<u64> = line
            .split_whitespace()
            .skip(1)
            .filter_map(|v| v.parse().ok())
            .collect();
        if values.len() < 4 {
            return None;
        }
        let idle = values[3];
        let total: u64 = values.iter().sum();
        Some((idle, total))
    };

    let before = read_stat();
    std::thread::sleep(Duration::from_millis(100));
    let after = read_stat();

    match (before, after) {
        (Some((idle1, total1)), Some((idle2, total2))) => {
            let total_diff = total2.saturating_sub(total1) as f32;
            let idle_diff = idle2.saturating_sub(idle1) as f32;
            if total_diff == 0.0 {
                0.0
            } else {
                (1.0 - idle_diff / total_diff) * 100.0
            }
        }
        _ => 0.0,
    }
}

fn read_disk_info() -> (f32, f32) {
    // Use statvfs for the root filesystem
    #[cfg(target_os = "linux")]
    {
        use std::ffi::CString;
        let path = CString::new("/").unwrap();
        let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } == 0 {
            let block_size = stat.f_frsize as u64;
            let total = (stat.f_blocks * block_size) as f32 / (1024.0 * 1024.0 * 1024.0);
            let free = (stat.f_bfree * block_size) as f32 / (1024.0 * 1024.0 * 1024.0);
            return (total - free, total);
        }
    }
    (0.0, 0.0)
}

// ── Dashboard Renderer ────────────────────────────────────────────────────────

/// Render configuration for the dashboard.
#[derive(Debug, Clone)]
pub struct DashboardConfig {
    pub refresh_interval_ms: u64,
    pub show_system_metrics: bool,
    pub show_tool_log: bool,
    pub max_log_lines: usize,
    pub color_theme: ColorTheme,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            refresh_interval_ms: 500,
            show_system_metrics: true,
            show_tool_log: true,
            max_log_lines: 50,
            color_theme: ColorTheme::Dark,
        }
    }
}

/// Color theme for the dashboard.
#[derive(Debug, Clone, PartialEq)]
pub enum ColorTheme {
    Dark,
    Light,
    Solarized,
}

/// Run the interactive TUI dashboard.
///
/// This function takes over the terminal and renders the dashboard until
/// the user presses 'q' or Ctrl+C.
///
/// Requires the `dashboard` feature flag.
#[cfg(feature = "dashboard")]
pub async fn run_dashboard(
    state: Arc<DashboardState>,
    config: DashboardConfig,
) -> Result<()> {
    use crossterm::{
        event::{self, Event, KeyCode, KeyModifiers},
        execute,
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    };
    use ratatui::{
        backend::CrosstermBackend,
        layout::{Constraint, Direction, Layout},
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Cell, Gauge, List, ListItem, Paragraph, Row, Table, Tabs},
        Terminal,
    };
    use std::io::stdout;

    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut selected_tab = 0usize;
    let tab_titles = ["Overview", "Devices", "Events", "System"];
    let refresh = Duration::from_millis(config.refresh_interval_ms);

    loop {
        // Refresh system metrics
        let state_clone = state.clone();
        tokio::task::spawn_blocking(move || state_clone.refresh_system_metrics()).await?;

        terminal.draw(|f| {
            let size = f.size();

            // Header + tabs
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3), // Header
                    Constraint::Length(3), // Tabs
                    Constraint::Min(0),    // Content
                    Constraint::Length(1), // Footer
                ])
                .split(size);

            // Header
            let header = Paragraph::new(Line::from(vec![
                Span::styled(
                    " ⚡ Oh-Ben-Claw ",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "Multi-Device AI Assistant",
                    Style::default().fg(Color::White),
                ),
            ]))
            .block(Block::default().borders(Borders::ALL).border_style(
                Style::default().fg(Color::Cyan),
            ));
            f.render_widget(header, chunks[0]);

            // Tabs
            let tabs = Tabs::new(tab_titles.iter().map(|t| Line::from(*t)).collect::<Vec<_>>())
                .select(selected_tab)
                .block(Block::default().borders(Borders::ALL))
                .highlight_style(
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                );
            f.render_widget(tabs, chunks[1]);

            // Content area
            match selected_tab {
                0 => render_overview(f, chunks[2], &state),
                1 => render_devices(f, chunks[2], &state),
                2 => render_events(f, chunks[2], &state, config.max_log_lines),
                3 => render_system(f, chunks[2], &state),
                _ => {}
            }

            // Footer
            let footer = Paragraph::new(Line::from(vec![
                Span::styled(" [Tab] ", Style::default().fg(Color::Yellow)),
                Span::raw("Switch panel  "),
                Span::styled("[q] ", Style::default().fg(Color::Yellow)),
                Span::raw("Quit  "),
                Span::styled("[r] ", Style::default().fg(Color::Yellow)),
                Span::raw("Refresh"),
            ]));
            f.render_widget(footer, chunks[3]);
        })?;

        // Handle input with timeout
        if event::poll(refresh)? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') => break,
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                    KeyCode::Tab => {
                        selected_tab = (selected_tab + 1) % tab_titles.len();
                    }
                    KeyCode::BackTab => {
                        selected_tab = selected_tab.checked_sub(1).unwrap_or(tab_titles.len() - 1);
                    }
                    KeyCode::Char('r') => {
                        // Force refresh
                    }
                    _ => {}
                }
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

// ── Render Functions ──────────────────────────────────────────────────────────

#[cfg(feature = "dashboard")]
fn render_overview(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    state: &Arc<DashboardState>,
) {
    use ratatui::{
        layout::{Constraint, Direction, Layout},
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::{Block, Borders, Gauge, Paragraph},
    };

    let agent = state.agent.lock().unwrap().clone();
    let system = state.system.lock().unwrap().clone();
    let nodes = state.nodes.lock().unwrap();
    let online_nodes = nodes.iter().filter(|n| n.status == "online").count();
    drop(nodes);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8),
            Constraint::Length(5),
            Constraint::Min(0),
        ])
        .split(area);

    // Agent status panel
    let status_color = if agent.is_busy {
        Color::Yellow
    } else if agent.is_running {
        Color::Green
    } else {
        Color::Red
    };

    let status_text = if agent.is_busy {
        "● PROCESSING"
    } else if agent.is_running {
        "● IDLE"
    } else {
        "○ STOPPED"
    };

    let agent_info = vec![
        Line::from(vec![
            Span::raw("  Status:   "),
            Span::styled(status_text, Style::default().fg(status_color).add_modifier(Modifier::BOLD)),
        ]),
        Line::from(format!("  Provider:  {} / {}", agent.provider, agent.model)),
        Line::from(format!("  Session:   {}", agent.current_session)),
        Line::from(format!("  Turns:     {}", agent.total_turns)),
        Line::from(format!("  Tool Calls: {}", agent.total_tool_calls)),
        Line::from(format!("  Nodes:     {online_nodes} online")),
    ];

    let agent_block = Paragraph::new(agent_info)
        .block(Block::default().title(" Agent ").borders(Borders::ALL).border_style(
            Style::default().fg(status_color),
        ));
    f.render_widget(agent_block, chunks[0]);

    // Memory gauge
    let mem_pct = if system.memory_total_mb > 0 {
        (system.memory_used_mb as f64 / system.memory_total_mb as f64) as f64
    } else {
        0.0
    };

    let mem_gauge = Gauge::default()
        .block(Block::default().title(" Memory ").borders(Borders::ALL))
        .gauge_style(Style::default().fg(Color::Cyan))
        .ratio(mem_pct)
        .label(format!(
            "{} MB / {} MB",
            system.memory_used_mb, system.memory_total_mb
        ));
    f.render_widget(mem_gauge, chunks[1]);
}

#[cfg(feature = "dashboard")]
fn render_devices(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    state: &Arc<DashboardState>,
) {
    use ratatui::{
        style::{Color, Modifier, Style},
        widgets::{Block, Borders, Cell, Row, Table},
    };

    let nodes = state.nodes.lock().unwrap();
    let rows: Vec<Row> = nodes
        .iter()
        .map(|n| {
            let status_color = match n.status.as_str() {
                "online" => Color::Green,
                "offline" => Color::Red,
                "paired" => Color::Cyan,
                _ => Color::Yellow,
            };
            Row::new(vec![
                Cell::from(n.name.clone()),
                Cell::from(n.board_type.clone()),
                Cell::from(n.status.clone()).style(Style::default().fg(status_color)),
                Cell::from(n.tool_count.to_string()),
                Cell::from(
                    n.latency_ms
                        .map(|l| format!("{l}ms"))
                        .unwrap_or_else(|| "-".to_string()),
                ),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            ratatui::layout::Constraint::Percentage(25),
            ratatui::layout::Constraint::Percentage(25),
            ratatui::layout::Constraint::Percentage(15),
            ratatui::layout::Constraint::Percentage(15),
            ratatui::layout::Constraint::Percentage(20),
        ],
    )
    .header(
        Row::new(vec!["Name", "Board", "Status", "Tools", "Latency"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().title(" Peripheral Nodes ").borders(Borders::ALL));

    f.render_widget(table, area);
}

#[cfg(feature = "dashboard")]
fn render_events(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    state: &Arc<DashboardState>,
    max_lines: usize,
) {
    use ratatui::{
        style::{Color, Style},
        text::{Line, Span},
        widgets::{Block, Borders, List, ListItem},
    };

    let events = state.events.lock().unwrap();
    let items: Vec<ListItem> = events
        .iter()
        .rev()
        .take(max_lines)
        .map(|e| {
            let level_color = match e.level {
                EventLevel::Info => Color::White,
                EventLevel::Success => Color::Green,
                EventLevel::Warning => Color::Yellow,
                EventLevel::Error => Color::Red,
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("[{}] ", e.level.label()),
                    Style::default().fg(level_color),
                ),
                Span::styled(
                    format!("{}: ", e.source),
                    Style::default().fg(Color::Cyan),
                ),
                Span::raw(e.message.clone()),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().title(" Event Log ").borders(Borders::ALL));
    f.render_widget(list, area);
}

#[cfg(feature = "dashboard")]
fn render_system(
    f: &mut ratatui::Frame,
    area: ratatui::layout::Rect,
    state: &Arc<DashboardState>,
) {
    use ratatui::{
        layout::{Constraint, Direction, Layout},
        style::{Color, Style},
        widgets::{Block, Borders, Gauge},
    };

    let system = state.system.lock().unwrap().clone();
    let uptime = state.uptime();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(area);

    let cpu_gauge = Gauge::default()
        .block(Block::default().title(" CPU ").borders(Borders::ALL))
        .gauge_style(Style::default().fg(if system.cpu_percent > 80.0 {
            Color::Red
        } else if system.cpu_percent > 50.0 {
            Color::Yellow
        } else {
            Color::Green
        }))
        .ratio((system.cpu_percent / 100.0) as f64)
        .label(format!("{:.1}%", system.cpu_percent));
    f.render_widget(cpu_gauge, chunks[0]);

    let mem_pct = if system.memory_total_mb > 0 {
        system.memory_used_mb as f64 / system.memory_total_mb as f64
    } else {
        0.0
    };
    let mem_gauge = Gauge::default()
        .block(Block::default().title(" Memory ").borders(Borders::ALL))
        .gauge_style(Style::default().fg(Color::Cyan))
        .ratio(mem_pct)
        .label(format!(
            "{} / {} MB",
            system.memory_used_mb, system.memory_total_mb
        ));
    f.render_widget(mem_gauge, chunks[1]);

    let disk_pct = if system.disk_total_gb > 0.0 {
        (system.disk_used_gb / system.disk_total_gb) as f64
    } else {
        0.0
    };
    let disk_gauge = Gauge::default()
        .block(Block::default().title(" Disk ").borders(Borders::ALL))
        .gauge_style(Style::default().fg(Color::Magenta))
        .ratio(disk_pct)
        .label(format!(
            "{:.1} / {:.1} GB",
            system.disk_used_gb, system.disk_total_gb
        ));
    f.render_widget(disk_gauge, chunks[2]);

    let uptime_secs = uptime.as_secs();
    let uptime_str = format!(
        "{}h {}m {}s",
        uptime_secs / 3600,
        (uptime_secs % 3600) / 60,
        uptime_secs % 60
    );
    let uptime_widget = ratatui::widgets::Paragraph::new(format!("  Uptime: {uptime_str}"))
        .block(Block::default().title(" Process ").borders(Borders::ALL));
    f.render_widget(uptime_widget, chunks[3]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dashboard_state_new() {
        let state = DashboardState::new();
        assert!(state.events.lock().unwrap().is_empty());
        assert!(state.nodes.lock().unwrap().is_empty());
    }

    #[test]
    fn test_push_event() {
        let state = DashboardState::new();
        state.push_event(EventLevel::Info, "test", "hello");
        state.push_event(EventLevel::Error, "agent", "something failed");
        let events = state.events.lock().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].level, EventLevel::Error);
    }

    #[test]
    fn test_push_event_max_capacity() {
        let mut state = DashboardState::new();
        state.max_events = 3;
        for i in 0..5 {
            state.push_event(EventLevel::Info, "test", &format!("event {i}"));
        }
        let events = state.events.lock().unwrap();
        assert_eq!(events.len(), 3);
        // Should have the last 3 events
        assert!(events[2].message.contains("event 4"));
    }

    #[test]
    fn test_update_node() {
        let state = DashboardState::new();
        let node = NodeStatus {
            name: "esp32-1".to_string(),
            board_type: "ESP32-S3".to_string(),
            status: "online".to_string(),
            tool_count: 5,
            last_seen: 0,
            latency_ms: Some(12),
            error: None,
        };
        state.update_node(node.clone());
        assert_eq!(state.nodes.lock().unwrap().len(), 1);

        // Update existing node
        let updated = NodeStatus {
            status: "offline".to_string(),
            ..node
        };
        state.update_node(updated);
        let nodes = state.nodes.lock().unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].status, "offline");
    }

    #[test]
    fn test_remove_node() {
        let state = DashboardState::new();
        state.update_node(NodeStatus {
            name: "node1".to_string(),
            board_type: "RPi".to_string(),
            status: "online".to_string(),
            tool_count: 3,
            last_seen: 0,
            latency_ms: None,
            error: None,
        });
        state.remove_node("node1");
        assert!(state.nodes.lock().unwrap().is_empty());
    }

    #[test]
    fn test_event_level_labels() {
        assert_eq!(EventLevel::Info.label(), "INFO");
        assert_eq!(EventLevel::Success.label(), "OK  ");
        assert_eq!(EventLevel::Warning.label(), "WARN");
        assert_eq!(EventLevel::Error.label(), "ERR ");
    }

    #[test]
    fn test_dashboard_config_default() {
        let config = DashboardConfig::default();
        assert_eq!(config.refresh_interval_ms, 500);
        assert!(config.show_system_metrics);
        assert_eq!(config.color_theme, ColorTheme::Dark);
    }
}
