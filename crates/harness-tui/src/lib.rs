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
use harness_core::markdown::{self, Block as MdBlock, Inline};
use harness_core::{AgentEvent, AgentHandle, Shown, UserModeFilter};
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
    /// Developer mode (`--debug`): show the full log. When false (the default),
    /// only show what a non-developer needs.
    pub debug: bool,
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
    debug: bool,
    filter: UserModeFilter,
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
            debug: config.debug,
            filter: UserModeFilter::new(),
        };
        app.ingest_history(&config.history);
        app
    }

    /// Replay the session log. In developer mode this is the raw transcript; in
    /// user mode it is collapsed to the relevant messages.
    fn ingest_history(&mut self, history: &[SessionEvent]) {
        for ev in history {
            if self.debug {
                self.ingest_history_event(ev);
            } else if let Some(shown) = self.filter.on_session(ev) {
                self.render_shown(shown);
            }
        }
        if !self.debug {
            if let Some(shown) = self.filter.flush() {
                self.render_shown(shown);
            }
        }
        if !history.is_empty() && self.debug {
            self.push_plain("── session restored from session.csv ──", Color::DarkGray);
        }
    }

    fn ingest_history_event(&mut self, ev: &SessionEvent) {
        match ev.event_type.as_str() {
            "user_message" => {
                self.push_line(label("you", Color::Cyan), &ev.output);
            }
            "assistant_message" => {
                if !ev.output.trim().is_empty() {
                    self.push_markdown(label("masse", Color::Green), &ev.output);
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

    /// Render one filter-approved message. Used in user mode.
    fn render_shown(&mut self, shown: Shown) {
        match shown {
            Shown::User(text) => self.push_line(label("you", Color::Cyan), &text),
            Shown::Assistant(text) => self.push_markdown(label("masse", Color::Green), &text),
            Shown::Error(message) => self.push_plain(format!("⚠ {message}"), Color::Red),
            Shown::Finished { summary, success } => {
                let (mark, color) = if success {
                    ("✔", Color::Green)
                } else {
                    ("✘", Color::Red)
                };
                self.push_plain(format!("{mark} {summary}"), color);
            }
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

    /// Render a Markdown message. The first visual line carries the `prefix`
    /// label; continuation lines are indented to line up under it.
    fn push_markdown(&mut self, prefix: Span<'static>, text: &str) {
        let mut lines = markdown_lines(&markdown::parse(text));
        if lines.is_empty() {
            self.push_line(prefix, text);
            return;
        }
        let mut first = vec![prefix, Span::raw(" ")];
        first.append(&mut lines[0].spans);
        lines[0].spans = first;
        for line in lines.iter_mut().skip(1) {
            let mut spans = vec![Span::raw("    ")];
            spans.append(&mut line.spans);
            line.spans = spans;
        }
        self.lines.extend(lines);
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
        if self.debug {
            self.push_developer_event(&event);
        } else if let Some(shown) = self.filter.on_agent(&event) {
            // `submit` already echoed the user's own line; only the assistant's
            // (and errors) need rendering here.
            if !matches!(shown, Shown::User(_)) {
                self.render_shown(shown);
            }
        }
        self.apply_status(&event);
        self.trim();
        self.scroll_back = 0;
    }

    /// Update the status line and running flag from an event, in both modes.
    fn apply_status(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::Retrying { attempt, max, .. } => {
                self.status = format!("retrying {attempt}/{max}");
            }
            AgentEvent::Finished { success, .. } => {
                self.status = if *success { "done" } else { "failed" }.into();
                self.running = false;
            }
            AgentEvent::Error(_) => {
                self.status = "error".into();
                self.running = false;
            }
            AgentEvent::TurnFinished => {
                self.running = false;
                self.status = "ready".into();
            }
            _ => {}
        }
    }

    /// Push every event into the transcript. Used in developer mode.
    fn push_developer_event(&mut self, event: &AgentEvent) {
        match event {
            AgentEvent::UserMessage(_) => {}
            AgentEvent::AssistantMessage(text) => {
                self.push_markdown(label("masse", Color::Green), text);
            }
            AgentEvent::ToolCall { name, args } => {
                self.push_line(
                    Span::styled(format!("  ⚙ {name}"), Style::default().fg(Color::Yellow)),
                    args,
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
                self.push_line(Span::styled(header, Style::default().fg(color)), output);
            }
            AgentEvent::Retrying {
                attempt,
                max,
                reason,
            } => {
                self.push_plain(format!("↻ retry {attempt}/{max}: {reason}"), Color::Yellow);
            }
            AgentEvent::Notice(message) => {
                self.push_plain(format!("• {message}"), Color::DarkGray);
            }
            AgentEvent::Finished { summary, success } => {
                let (mark, color) = if *success {
                    ("✔", Color::Green)
                } else {
                    ("✘", Color::Red)
                };
                self.push_plain(format!("{mark} finished: {summary}"), color);
            }
            AgentEvent::Error(message) => {
                self.push_plain(format!("error: {message}"), Color::Red);
            }
            AgentEvent::TurnFinished => {}
        }
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
            if app.running {
                // Abort an in-flight turn instead of being stuck.
                handle.cancel();
                app.status = "cancelling".into();
            } else {
                app.input.clear();
                app.cursor = 0;
                app.history_cursor = None;
            }
        }
        _ => {}
    }
}

fn ui(frame: &mut Frame, app: &mut App, config: &TuiConfig) {
    let area = frame.area();

    // Grow the message box with its content instead of clipping it: measure how
    // many wrapped rows the input needs and size the box to match. The cap keeps
    // the transcript visible on short terminals and stops a wall of text from
    // swallowing the screen.
    let input_text: String = app.input.iter().collect();
    let input_inner_width = area.width.saturating_sub(2).max(1) as usize;
    let max_input_lines = (area.height as usize).saturating_sub(6).clamp(1, 10);
    let (input_lines, input_first_line) = input_box_rows(
        input_text.chars().count(),
        input_inner_width,
        max_input_lines,
    );
    let input_height = input_lines as u16 + 2; // top and bottom borders

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(input_height),
            Constraint::Length(1),
        ])
        .split(area);

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

    let transcript_title = if app.debug {
        format!(" masse · {} ", config.root)
    } else {
        " masse ".to_string()
    };
    let transcript = Paragraph::new(app.lines.clone())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(transcript_title)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0));
    frame.render_widget(transcript, chunks[0]);

    // Input
    let input = Paragraph::new(input_text)
        .style(Style::default().fg(Color::White))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" message (Enter to send · Esc cancel · Ctrl-C quit) ")
                .border_style(Style::default().fg(Color::Blue)),
        )
        .wrap(Wrap { trim: false })
        .scroll((input_first_line as u16, 0));
    frame.render_widget(input, chunks[1]);

    // Place the real terminal cursor inside the input box. The box wraps by
    // character (see `visual_lines`), so the cursor math matches the height
    // computation above, including when the text is scrolled up.
    let cursor = app.cursor.min(app.input.len());
    let cursor_row = (cursor / input_inner_width).saturating_sub(input_first_line);
    let cursor_col = (cursor % input_inner_width) as u16;
    let max_x = chunks[1].x + chunks[1].width.saturating_sub(2);
    let cursor_x = (chunks[1].x + 1 + cursor_col).min(max_x);
    let max_y = chunks[1].y + input_height.saturating_sub(2);
    let cursor_y = (chunks[1].y + 1 + cursor_row as u16).min(max_y);
    frame.set_cursor_position((cursor_x, cursor_y));

    // Status
    let spinner = if app.running {
        const FRAMES: [&str; 4] = ["|", "/", "-", "\\"];
        FRAMES[app.frame % FRAMES.len()]
    } else {
        "·"
    };
    let mut status_spans = vec![Span::styled(
        format!(" {spinner} {} ", app.status),
        Style::default().fg(if app.running {
            Color::Yellow
        } else {
            Color::Green
        }),
    )];
    if app.debug {
        status_spans.push(Span::styled(
            format!("| {} / {} ", config.provider_name, config.model),
            Style::default().fg(Color::DarkGray),
        ));
        status_spans.push(Span::styled(
            format!("| {} tools ", config.tool_count),
            Style::default().fg(Color::DarkGray),
        ));
        status_spans.push(Span::styled(
            format!("| {} ", config.session_path),
            Style::default().fg(Color::DarkGray),
        ));
        status_spans.push(Span::styled(
            format!("| scroll {} ", scroll),
            Style::default().fg(Color::DarkGray),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(status_spans)), chunks[2]);
}

fn visual_lines(width: usize, inner: usize) -> usize {
    if width == 0 {
        return 1;
    }
    (width + inner - 1) / inner.max(1)
}

/// Rows the message box should show for an input of `input_chars` characters,
/// and the index of the first visible wrapped row. When the content is taller
/// than `max_rows`, the box scrolls to the end so the cursor stays in view.
fn input_box_rows(input_chars: usize, inner_width: usize, max_rows: usize) -> (usize, usize) {
    let total = visual_lines(input_chars, inner_width).max(1);
    let visible = total.min(max_rows.max(1));
    (visible, total - visible)
}

/// Render parsed Markdown blocks to styled terminal lines.
fn markdown_lines(blocks: &[MdBlock]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for block in blocks {
        push_block(&mut lines, block);
    }
    lines
}

fn push_block(lines: &mut Vec<Line<'static>>, block: &MdBlock) {
    match block {
        MdBlock::Heading { level, content } => {
            let color = match level {
                1 => Color::LightCyan,
                2 => Color::LightGreen,
                _ => Color::LightYellow,
            };
            let style = Style::default().fg(color).add_modifier(Modifier::BOLD);
            lines.push(Line::from(inline_spans(content, style)));
            if *level <= 2 {
                lines.push(Line::from(Span::styled(
                    "─".repeat(30),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
        MdBlock::Paragraph(content) => {
            lines.push(Line::from(inline_spans(
                content,
                Style::default().fg(Color::Gray),
            )));
        }
        MdBlock::CodeBlock { code, .. } => {
            for raw in code.split('\n') {
                lines.push(Line::from(Span::styled(
                    format!("  {raw}"),
                    Style::default().fg(Color::LightYellow),
                )));
            }
        }
        MdBlock::Quote(inner) => {
            let mut sub = Vec::new();
            for block in inner {
                push_block(&mut sub, block);
            }
            for mut line in sub {
                let mut spans = vec![Span::styled("│ ", Style::default().fg(Color::DarkGray))];
                spans.append(&mut line.spans);
                lines.push(Line::from(spans));
            }
        }
        MdBlock::List {
            ordered,
            start,
            items,
        } => {
            for (index, item) in items.iter().enumerate() {
                let marker = if *ordered {
                    format!("{}. ", start + index as u64)
                } else {
                    "• ".to_string()
                };
                let mut spans = vec![Span::styled(marker, Style::default().fg(Color::Green))];
                spans.extend(inline_spans(item, Style::default().fg(Color::Gray)));
                lines.push(Line::from(spans));
            }
        }
        MdBlock::Rule => {
            lines.push(Line::from(Span::styled(
                "─".repeat(40),
                Style::default().fg(Color::DarkGray),
            )));
        }
        MdBlock::Table { header, rows } => push_table(lines, header, rows),
    }
}

fn push_table(lines: &mut Vec<Line<'static>>, header: &[Vec<Inline>], rows: &[Vec<Vec<Inline>>]) {
    let cols = header.len();
    let mut widths = vec![0usize; cols];
    for (c, cell) in header.iter().enumerate() {
        widths[c] = markdown::inline_text(cell).chars().count();
    }
    for row in rows {
        for (c, cell) in row.iter().enumerate().take(cols) {
            widths[c] = widths[c].max(markdown::inline_text(cell).chars().count());
        }
    }

    let mut header_spans = Vec::new();
    for (c, cell) in header.iter().enumerate() {
        if c > 0 {
            header_spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
        }
        header_spans.extend(inline_spans(
            cell,
            Style::default()
                .fg(Color::LightCyan)
                .add_modifier(Modifier::BOLD),
        ));
        header_spans.push(Span::styled(
            " ".repeat(widths[c].saturating_sub(markdown::inline_text(cell).chars().count())),
            Style::default().fg(Color::Gray),
        ));
    }
    lines.push(Line::from(header_spans));

    let separator = widths
        .iter()
        .map(|w| "─".repeat(*w))
        .collect::<Vec<_>>()
        .join("─┼─");
    lines.push(Line::from(Span::styled(
        separator,
        Style::default().fg(Color::DarkGray),
    )));

    for row in rows {
        let mut spans = Vec::new();
        for (c, width) in widths.iter().enumerate() {
            if c > 0 {
                spans.push(Span::styled(" │ ", Style::default().fg(Color::DarkGray)));
            }
            let cell = row.get(c);
            if let Some(cell) = cell {
                spans.extend(inline_spans(cell, Style::default().fg(Color::Gray)));
            }
            let text = cell
                .map(|cell| markdown::inline_text(cell))
                .unwrap_or_default();
            spans.push(Span::styled(
                " ".repeat(width.saturating_sub(text.chars().count())),
                Style::default().fg(Color::Gray),
            ));
        }
        lines.push(Line::from(spans));
    }
}

fn inline_spans(inlines: &[Inline], base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for inline in inlines {
        match inline {
            Inline::Text(t) => spans.push(Span::styled(t.clone(), base)),
            Inline::Code(t) => spans.push(Span::styled(
                t.clone(),
                Style::default().fg(Color::LightYellow),
            )),
            Inline::Bold(x) => spans.extend(inline_spans(x, base.add_modifier(Modifier::BOLD))),
            Inline::Italic(x) => spans.extend(inline_spans(x, base.add_modifier(Modifier::ITALIC))),
            Inline::Strike(x) => {
                spans.extend(inline_spans(x, base.add_modifier(Modifier::CROSSED_OUT)))
            }
            Inline::Link { text, url } => {
                spans.extend(inline_spans(
                    text,
                    base.fg(Color::LightCyan).add_modifier(Modifier::UNDERLINED),
                ));
                if markdown::inline_text(text).trim() != url {
                    spans.push(Span::styled(
                        format!(" ({url})"),
                        Style::default().fg(Color::DarkGray),
                    ));
                }
            }
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_box_shows_one_row_when_empty_or_short() {
        assert_eq!(input_box_rows(0, 10, 10), (1, 0));
        assert_eq!(input_box_rows(10, 10, 10), (1, 0));
    }

    #[test]
    fn input_box_grows_with_wrapped_rows() {
        // One character over the edge wraps onto a second row.
        assert_eq!(input_box_rows(11, 10, 10), (2, 0));
        assert_eq!(input_box_rows(25, 10, 10), (3, 0));
    }

    #[test]
    fn input_box_caps_and_scrolls_to_the_end() {
        // 35 chars on a 10-wide box is 4 rows; a cap of 2 shows the last two.
        assert_eq!(input_box_rows(35, 10, 2), (2, 2));
    }

    #[test]
    fn input_box_never_shrinks_below_one_row() {
        assert_eq!(input_box_rows(0, 1, 0), (1, 0));
    }

    fn app(debug: bool) -> App {
        App::new(&TuiConfig {
            debug,
            ..Default::default()
        })
    }

    fn transcript(app: &App) -> String {
        app.lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn user_mode_keeps_only_the_final_message_and_hides_tools() {
        let mut app = app(false);
        app.on_agent_event(AgentEvent::AssistantMessage("paso 1".into()));
        app.on_agent_event(AgentEvent::ToolCall {
            name: "read_file".into(),
            args: "{}".into(),
        });
        app.on_agent_event(AgentEvent::AssistantMessage("listo, quedó andando".into()));
        app.on_agent_event(AgentEvent::TurnFinished);
        let text = transcript(&app);
        assert!(text.contains("listo, quedó andando"));
        assert!(!text.contains("paso 1"));
        assert!(!text.contains("read_file"));
    }

    #[test]
    fn debug_mode_keeps_everything() {
        let mut app = app(true);
        app.on_agent_event(AgentEvent::AssistantMessage("paso 1".into()));
        app.on_agent_event(AgentEvent::ToolCall {
            name: "read_file".into(),
            args: "{}".into(),
        });
        app.on_agent_event(AgentEvent::AssistantMessage("listo".into()));
        let text = transcript(&app);
        assert!(text.contains("paso 1"));
        assert!(text.contains("read_file"));
        assert!(text.contains("listo"));
    }

    #[test]
    fn assistant_markdown_is_rendered_not_shown_raw() {
        let mut app = app(false);
        app.on_agent_event(AgentEvent::AssistantMessage(
            "# Result\n\nThis is **bold** and `code` here.".into(),
        ));
        app.on_agent_event(AgentEvent::TurnFinished);
        let text = transcript(&app);
        assert!(text.contains("Result"));
        assert!(text.contains("bold"));
        assert!(text.contains("code"));
        // Markup is consumed, not printed verbatim.
        assert!(!text.contains("**"));
        assert!(!text.contains("# Result"));
    }

    #[test]
    fn code_blocks_and_lists_render_each_line() {
        let mut app = app(true);
        app.on_agent_event(AgentEvent::AssistantMessage(
            "- one\n- two\n\n```sh\nls -la\n```".into(),
        ));
        let text = transcript(&app);
        assert!(text.contains("• one"));
        assert!(text.contains("• two"));
        assert!(text.contains("ls -la"));
    }

    #[test]
    fn user_mode_replays_history_as_questions_and_summaries() {
        let mut app = app(false);
        app.ingest_history(&[
            SessionEvent {
                event_type: "user_message".into(),
                output: "quiero una app".into(),
                ..Default::default()
            },
            SessionEvent {
                event_type: "assistant_message".into(),
                output: "Voy a revisar...".into(),
                ..Default::default()
            },
            SessionEvent {
                event_type: "tool_call".into(),
                tool: "read_file".into(),
                ..Default::default()
            },
            SessionEvent {
                event_type: "assistant_message".into(),
                output: "¿Preferís A o B?".into(),
                ..Default::default()
            },
        ]);
        let text = transcript(&app);
        assert!(text.contains("quiero una app"));
        assert!(text.contains("¿Preferís A o B?"));
        assert!(!text.contains("Voy a revisar"));
        assert!(!text.contains("read_file"));
    }
}
