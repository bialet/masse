//! The ratatui terminal front end.
//!
//! The TUI is a thin view over [`harness_core::AgentHandle`]: it renders the
//! session transcript, forwards typed messages to the actor and renders the
//! streamed [`harness_core::AgentEvent`]s as they arrive.

use std::io::IsTerminal;
use std::time::Duration;

use anyhow::{bail, Result};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use harness_core::{AgentEvent, AgentHandle};
use harness_session::Event as SessionEvent;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use tokio::sync::broadcast::error::RecvError;

const MAX_TRANSCRIPT_LINES: usize = 4000;

/// Everything the TUI needs that is not the live agent.
#[derive(Clone, Debug, Default)]
pub struct TuiConfig {
    pub provider_name: String,
    pub model: String,
    pub session_path: String,
    pub tool_count: usize,
    pub history: Vec<SessionEvent>,
    pub root: String,
}

struct App {
    lines: Vec<Line<'static>>,
    input: Vec<char>,
    cursor: usize,
    scroll_back: u16,
    status: String,
    running: bool,
    frame: usize,
    should_quit: bool,
    input_history: Vec<String>,
    history_cursor: Option<usize>,
}

impl App {
    fn new(config: &TuiConfig) -> Self {
        let mut app = App {
            lines: Vec::new(),
            input: Vec::new(),
            cursor: 0,
            scroll_back: 0,
            status: "ready".to_string(),
            running: false,
            frame: 0,
            should_quit: false,
            input_history: Vec::new(),
            history_cursor: None,
        };
        app.ingest_history(&config.history);
        app
    }

    fn ingest_history(&mut self, history: &[SessionEvent]) {
        for ev in history {
            match ev.event_type.as_str() {
                "user_message" => {
                    self.push_line(label("you", Color::Cyan), &ev.output);
                }
                "assistant_message" => {
                    if !ev.output.trim().is_empty() {
                        self.push_line(label("masse", Color::Green), &ev.output);
                    }
                }
                "tool_call" => {
                    self.push_line(
                        Span::styled(
                            format!("  ⚙ {}", ev.tool),
                            Style::default().fg(Color::Yellow),
                        ),
                        &ev.args,
                    );
                }
                "tool_result" => {
                    let ok = ev.status == "ok";
                    let (mark, color) = if ok {
                        ("✔", Color::Green)
                    } else {
                        ("✘", Color::Red)
                    };
                    let header = format!(
                        "  {mark} {} (exit {})",
                        ev.tool,
                        if ev.exit_code.is_empty() {
                            "-"
                        } else {
                            &ev.exit_code
                        }
                    );
                    self.push_line(Span::styled(header, Style::default().fg(color)), &ev.output);
                }
                "session_start" | "session_end" => {}
                _ => {}
            }
        }
        if !history.is_empty() {
            self.push_plain("── session restored from session.csv ──", Color::DarkGray);
        }
    }

    fn push_line(&mut self, prefix: Span<'static>, text: &str) {
        let mut first = true;
        for raw in text.split('\n') {
            let mut spans = Vec::new();
            if first {
                spans.push(prefix.clone());
                spans.push(Span::raw(" "));
                first = false;
            } else {
                spans.push(Span::raw("    "));
            }
            spans.push(Span::styled(
                raw.to_string(),
                Style::default().fg(Color::Gray),
            ));
            self.lines.push(Line::from(spans));
        }
    }

    fn push_plain(&mut self, text: impl Into<String>, color: Color) {
        self.lines.push(Line::from(Span::styled(
            text.into(),
            Style::default().fg(color),
        )));
    }

    fn trim(&mut self) {
        if self.lines.len() > MAX_TRANSCRIPT_LINES {
            let drop = self.lines.len() - MAX_TRANSCRIPT_LINES;
            self.lines.drain(0..drop);
        }
    }

    fn on_agent_event(&mut self, event: AgentEvent) {
        match event {
            AgentEvent::UserMessage(_) => {}
            AgentEvent::AssistantMessage(text) => {
                self.push_line(label("masse", Color::Green), &text);
            }
            AgentEvent::ToolCall { name, args } => {
                self.push_line(
                    Span::styled(format!("  ⚙ {name}"), Style::default().fg(Color::Yellow)),
                    &args,
                );
            }
            AgentEvent::ToolResult {
                name,
                status,
                output,
                exit_code,
            } => {
                let ok = status == "ok";
                let (mark, color) = if ok {
                    ("✔", Color::Green)
                } else {
                    ("✘", Color::Red)
                };
                let header = format!(
                    "  {mark} {name} (exit {})",
                    exit_code
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "-".into())
                );
                self.push_line(Span::styled(header, Style::default().fg(color)), &output);
            }
            AgentEvent::Error(message) => {
                self.push_plain(format!("error: {message}"), Color::Red);
                self.status = "error".into();
                self.running = false;
            }
            AgentEvent::TurnFinished => {
                self.running = false;
                self.status = "ready".into();
            }
        }
        self.trim();
        self.scroll_back = 0;
    }

    fn submit(&mut self, handle: &AgentHandle) {
        let text: String = self.input.iter().collect();
        let text = text.trim().to_string();
        if text.is_empty() || self.running {
            return;
        }
        self.push_line(label("you", Color::Cyan), &text);
        self.input.clear();
        self.cursor = 0;
        self.scroll_back = 0;
        self.running = true;
        self.status = "thinking".into();
        self.input_history.push(text.clone());
        self.history_cursor = None;
        if !handle.send_user(text) {
            self.push_plain("agent actor has stopped", Color::Red);
            self.running = false;
            self.status = "error".into();
        }
    }
}

fn label(text: &str, color: Color) -> Span<'static> {
    Span::styled(
        format!("{text}:"),
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    )
}

/// Run the TUI until the user quits.
pub async fn run(config: TuiConfig, handle: AgentHandle) -> Result<()> {
    // The TUI needs a real terminal; otherwise fail loudly instead of hanging.
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        bail!(
            "--tui requires an interactive terminal (stdin/stdout are not a TTY). \
             Use --serve for a non-interactive environment."
        );
    }

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Read keys on a dedicated blocking thread. This is the most portable way to
    // receive input across terminals (macOS Terminal, iTerm2, tmux, Linux TTYs)
    // and avoids the async EventStream, which can silently deliver nothing on
    // some platforms.
    let (key_tx, mut key_rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    std::thread::spawn(move || {
        while let Ok(event) = crossterm::event::read() {
            if key_tx.send(event).is_err() {
                break;
            }
        }
    });

    let result = run_loop(&mut terminal, &config, handle, &mut key_rx).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

async fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    config: &TuiConfig,
    handle: AgentHandle,
    key_rx: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
) -> Result<()> {
    let mut app = App::new(config);
    let mut agent_rx = handle.subscribe();
    let mut ticker = tokio::time::interval(Duration::from_millis(200));

    loop {
        terminal.draw(|f| ui(f, &mut app, config))?;
        if app.should_quit {
            break;
        }
        tokio::select! {
            maybe_event = key_rx.recv() => {
                match maybe_event {
                    Some(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                        handle_key(&mut app, &handle, key);
                    }
                    Some(_) => {}
                    None => app.should_quit = true,
                }
            }
            received = agent_rx.recv() => {
                match received {
                    Ok(event) => app.on_agent_event(event),
                    Err(RecvError::Lagged(n)) => {
                        app.push_plain(format!("dropped {n} events"), Color::DarkGray);
                    }
                    Err(RecvError::Closed) => app.should_quit = true,
                }
            }
            _ = ticker.tick() => {
                app.frame = app.frame.wrapping_add(1);
            }
        }
    }
    Ok(())
}

fn handle_key(app: &mut App, handle: &AgentHandle, key: KeyEvent) {
    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        KeyCode::Enter => app.submit(handle),
        KeyCode::Backspace => {
            if app.cursor > 0 {
                app.cursor -= 1;
                app.input.remove(app.cursor);
            }
        }
        KeyCode::Delete => {
            if app.cursor < app.input.len() {
                app.input.remove(app.cursor);
            }
        }
        KeyCode::Left => app.cursor = app.cursor.saturating_sub(1),
        KeyCode::Right => {
            if app.cursor < app.input.len() {
                app.cursor += 1;
            }
        }
        KeyCode::Home => app.cursor = 0,
        KeyCode::End => app.cursor = app.input.len(),
        KeyCode::Char(c) => {
            app.input.insert(app.cursor, c);
            app.cursor += 1;
        }
        KeyCode::PageUp => app.scroll_back = app.scroll_back.saturating_add(5),
        KeyCode::PageDown => app.scroll_back = app.scroll_back.saturating_sub(5),
        KeyCode::Up => {
            if app.input_history.is_empty() {
                return;
            }
            let idx = match app.history_cursor {
                Some(0) => 0,
                Some(i) => i - 1,
                None => app.input_history.len() - 1,
            };
            app.history_cursor = Some(idx);
            app.input = app.input_history[idx].chars().collect();
            app.cursor = app.input.len();
        }
        KeyCode::Down => {
            if let Some(i) = app.history_cursor {
                if i + 1 < app.input_history.len() {
                    app.history_cursor = Some(i + 1);
                    app.input = app.input_history[i + 1].chars().collect();
                } else {
                    app.history_cursor = None;
                    app.input.clear();
                }
                app.cursor = app.input.len();
            }
        }
        KeyCode::Esc => {
            app.input.clear();
            app.cursor = 0;
            app.history_cursor = None;
        }
        _ => {}
    }
}

fn ui(frame: &mut Frame, app: &mut App, config: &TuiConfig) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(frame.area());

    // Transcript
    let inner_width = chunks[0].width.saturating_sub(2).max(1) as usize;
    let inner_height = chunks[0].height.saturating_sub(2) as usize;
    let total_visual: usize = app
        .lines
        .iter()
        .map(|l| visual_lines(l.width(), inner_width))
        .sum();
    let max_scroll = total_visual.saturating_sub(inner_height);
    let scroll = max_scroll.saturating_sub(app.scroll_back as usize) as u16;

    let transcript = Paragraph::new(app.lines.clone())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" masse · {} ", config.root))
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(transcript, chunks[0]);

    // Input
    let input_text: String = app.input.iter().collect();
    let input = Paragraph::new(input_text)
        .style(Style::default().fg(Color::White))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" message (Enter to send · Ctrl-C to quit) ")
                .border_style(Style::default().fg(Color::Blue)),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(input, chunks[1]);

    // Place the real terminal cursor inside the input box.
    let max_x = chunks[1].x + chunks[1].width.saturating_sub(2);
    let cursor_x = (chunks[1].x + 1 + app.input[..app.cursor].len() as u16).min(max_x);
    frame.set_cursor_position((cursor_x, chunks[1].y + 1));

    // Status
    let spinner = if app.running {
        const FRAMES: [&str; 4] = ["|", "/", "-", "\\"];
        FRAMES[app.frame % FRAMES.len()]
    } else {
        "·"
    };
    let status = Line::from(vec![
        Span::styled(
            format!(" {spinner} {} ", app.status),
            Style::default().fg(if app.running {
                Color::Yellow
            } else {
                Color::Green
            }),
        ),
        Span::styled(
            format!("| {} / {} ", config.provider_name, config.model),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            format!("| {} tools ", config.tool_count),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            format!("| {} ", config.session_path),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            format!("| scroll {} ", scroll),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    frame.render_widget(Paragraph::new(status), chunks[2]);
}

fn visual_lines(width: usize, inner: usize) -> usize {
    if width == 0 {
        return 1;
    }
    (width + inner - 1) / inner.max(1)
}
