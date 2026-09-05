mod native_glyph;
mod unicode_strokes;

use crate::model::AgentKind;
use alacritty_terminal::{
    event::{Event, EventListener},
    grid::{Dimensions, Scroll},
    index::{Column, Line, Point as TerminalPoint, Side},
    selection::{Selection, SelectionType},
    term::{Config as AlacrittyConfig, Term, TermMode, cell::Flags, search::RegexSearch},
    vte::ansi::{Color, CursorShape, NamedColor, Processor},
};
use gpui::{
    App, Bounds, ClipboardItem, Context, DispatchPhase, Edges, ElementInputHandler, Entity,
    EntityInputHandler, FocusHandle, Focusable as _, Font, FontFeatures, FontStyle, FontWeight,
    Hsla, IntoElement, KeyBinding, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement as _, Pixels, Point, Render, ScrollWheelEvent, ShapedLine,
    SharedString, Size, StrikethroughStyle, Styled as _, Subscription, Task, TextRun, Timer,
    UTF16Selection, UnderlineStyle, WeakEntity, Window, canvas, div, prelude::*, px, quad, rgb,
    size, transparent_black,
};
use gpui_component::{
    Disableable as _, IconName, Selectable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
};
use gpui_terminal::{
    TerminalConfig, TerminalEvent, TerminalRenderer,
    input::keystroke_to_bytes,
    mouse::{encode_modifiers, mouse_button_report, scroll_report},
};
use parking_lot::Mutex;
use regex::Regex;
use std::{
    io::{Read, Write},
    ops::Range,
    sync::LazyLock,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

type ResizeCallback = dyn Fn(usize, usize) + Send + Sync;
type BellCallback = Box<dyn Fn(&mut Context<FastTerminalView>)>;
type TitleCallback = Box<dyn Fn(&mut Context<FastTerminalView>, &str)>;
type ExitCallback = Box<dyn Fn(&mut Context<FastTerminalView>)>;
type ClipboardCallback = Box<dyn Fn(&mut Context<FastTerminalView>, &str)>;
type AgentCallback = Box<dyn Fn(AgentTerminalSignal, &mut Context<FastTerminalView>)>;
type ScreenModeCallback = Box<dyn Fn(bool, &mut Context<FastTerminalView>)>;

const AGENT_OUTPUT_SETTLE_DELAY: Duration = Duration::from_secs(4);
const TERMINAL_KEY_CONTEXT: &str = "BlackholesTerminal";
const MAX_TERMINAL_SEARCH_MATCHES: usize = 20_000;

gpui::actions!(blackholes_terminal, [SendTab, SendBackTab]);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentTerminalSignalKind {
    Started,
    Working,
    Attention,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentTerminalSignal {
    pub agent: Option<AgentKind>,
    pub kind: AgentTerminalSignalKind,
}

enum FastTerminalEvent {
    Terminal(TerminalEvent),
    Agent(AgentTerminalSignal),
    AlternateScreen(bool),
}

enum SettleDecision {
    Wait(Duration),
    Stop,
}

struct FastEventProxy {
    sender: flume::Sender<FastTerminalEvent>,
}

impl FastEventProxy {
    fn send(&self, event: TerminalEvent) {
        let _ = self.sender.try_send(FastTerminalEvent::Terminal(event));
    }
}

impl EventListener for FastEventProxy {
    fn send_event(&self, event: Event) {
        match event {
            Event::Bell => self.send(TerminalEvent::Bell),
            Event::Title(title) => self.send(TerminalEvent::Title(title)),
            Event::ResetTitle => self.send(TerminalEvent::Title(String::new())),
            Event::ClipboardStore(_, text) => self.send(TerminalEvent::ClipboardStore(text)),
            Event::ClipboardLoad(_, _) => self.send(TerminalEvent::ClipboardLoad),
            Event::Exit | Event::ChildExit(_) => self.send(TerminalEvent::Exit),
            Event::Wakeup
            | Event::MouseCursorDirty
            | Event::PtyWrite(_)
            | Event::ColorRequest(_, _)
            | Event::TextAreaSizeRequest(_)
            | Event::CursorBlinkingChange => {}
        }
    }
}

/// Blackholes' bounded, scrollable GPUI terminal view.
///
/// The upstream view provides the parser and renderer building blocks, but its
/// output queue is unbounded and its mouse/scroll handlers are placeholders.
/// This implementation parses each 8 KiB PTY read on its own worker, coalesces
/// repaint requests, and supports native scrollback, TUI mouse-wheel reports,
/// selection, copy, and bracketed paste.
pub struct FastTerminalView {
    term: Arc<Mutex<Term<FastEventProxy>>>,
    renderer: TerminalRenderer,
    renderer_measured: bool,
    config: TerminalConfig,
    focus_handle: FocusHandle,
    stdin_writer: Arc<Mutex<Box<dyn Write + Send>>>,
    event_rx: flume::Receiver<FastTerminalEvent>,
    _reader_task: Task<()>,
    _focus_subscriptions: Vec<Subscription>,
    resize_callback: Option<Arc<Box<ResizeCallback>>>,
    bell_callback: Option<BellCallback>,
    title_callback: Option<TitleCallback>,
    exit_callback: Option<ExitCallback>,
    clipboard_callback: Option<ClipboardCallback>,
    agent_callback: Option<AgentCallback>,
    screen_mode_callback: Option<ScreenModeCallback>,
    geometry: Arc<Mutex<TerminalGeometry>>,
    paint_state: Arc<Mutex<TerminalPaintState>>,
    render_revision: Arc<AtomicU64>,
    selecting: bool,
    /// Last cell the in-flight selection was extended to, so a drag only
    /// repaints when it crosses a cell boundary.
    last_selection_point: Option<TerminalPoint>,
    /// Text the macOS input method is still composing (a CJK candidate
    /// window). It is not in the grid yet: it only reaches the PTY once the
    /// IME commits it.
    ime_marked: Option<String>,
    search_input: Entity<InputState>,
    search_open: bool,
    search_regex: bool,
    search_case_sensitive: bool,
    search_pattern_valid: bool,
    search_truncated: bool,
    search_matches: Vec<TerminalSearchMatch>,
    search_current: Option<usize>,
    search_saved_selection: Option<Selection>,
    /// A press a TUI asked to receive, held back until we know whether the
    /// pointer moves. Released as a click, discarded once it becomes a drag.
    pending_mouse_report: Option<(MouseButton, Vec<u8>)>,
    agent_turn_pending: bool,
    agent_output_seen: bool,
    agent_settle_running: bool,
    last_agent_output: Option<Instant>,
}

#[derive(Clone, Copy)]
struct TerminalGeometry {
    bounds: Bounds<Pixels>,
    cell_width: Pixels,
    cell_height: Pixels,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct TerminalSearchMatch {
    start: TerminalPoint,
    end: TerminalPoint,
}

impl Default for TerminalGeometry {
    fn default() -> Self {
        Self {
            bounds: Bounds::default(),
            cell_width: px(8.),
            cell_height: px(16.),
        }
    }
}

#[derive(Default)]
struct OscScanner {
    buffer: Vec<u8>,
    in_osc: bool,
    escape_pending: bool,
}

impl OscScanner {
    fn scan(&mut self, bytes: &[u8], mut on_signal: impl FnMut(AgentTerminalSignal)) {
        for &byte in bytes {
            if !self.in_osc {
                if self.escape_pending {
                    self.escape_pending = false;
                    if byte == b']' {
                        self.in_osc = true;
                        self.buffer.clear();
                    } else if byte == b'\x1b' {
                        self.escape_pending = true;
                    }
                } else if byte == b'\x1b' {
                    self.escape_pending = true;
                }
                continue;
            }

            if self.escape_pending {
                self.escape_pending = false;
                if byte == b'\\' {
                    self.finish(&mut on_signal);
                    continue;
                }
                self.buffer.push(b'\x1b');
                if byte == b'\x1b' {
                    self.escape_pending = true;
                } else {
                    self.buffer.push(byte);
                }
            } else {
                match byte {
                    b'\x07' => self.finish(&mut on_signal),
                    b'\x1b' => self.escape_pending = true,
                    _ => self.buffer.push(byte),
                }
            }

            if self.buffer.len() > 64 * 1024 {
                self.buffer.clear();
                self.in_osc = false;
                self.escape_pending = false;
            }
        }
    }

    fn finish(&mut self, on_signal: &mut impl FnMut(AgentTerminalSignal)) {
        if let Some(signal) = parse_agent_osc(&String::from_utf8_lossy(&self.buffer)) {
            on_signal(signal);
        }
        self.buffer.clear();
        self.in_osc = false;
        self.escape_pending = false;
    }
}

fn parse_agent_osc(sequence: &str) -> Option<AgentTerminalSignal> {
    if sequence.strip_prefix("9;").is_some() {
        return Some(AgentTerminalSignal {
            agent: None,
            kind: AgentTerminalSignalKind::Attention,
        });
    }

    let mut fields = sequence.splitn(4, ';');
    if fields.next()? != "777" || fields.next()? != "notify" {
        return None;
    }
    let title = fields.next()?.trim();
    let body = fields.next().unwrap_or_default();

    if title == "warp://cli-agent" {
        let payload: serde_json::Value = serde_json::from_str(body).ok()?;
        let kind = match payload.get("event").and_then(serde_json::Value::as_str)? {
            "session_start" | "session_resume" => AgentTerminalSignalKind::Started,
            "prompt_submit" | "tool_start" | "post_tool_use" | "tool_complete" => {
                AgentTerminalSignalKind::Working
            }
            "stop" | "stop_failure" | "permission_request" | "idle_prompt" | "question"
            | "elicitation_dialog" => AgentTerminalSignalKind::Attention,
            _ => return None,
        };
        // Structured Warp events are useful lifecycle signals, but their `agent` field is not
        // authoritative. Codex can load both codex-warp and claude-code-warp from the same
        // profile, causing both plugins to describe the same foreground session with different
        // identities. Blackholes determines identity from its own session hook, the selected
        // launcher, or an unambiguous terminal title instead.
        return Some(AgentTerminalSignal { agent: None, kind });
    }

    let agent = agent_from_protocol_name(title)?;
    Some(AgentTerminalSignal {
        agent: Some(agent),
        kind: AgentTerminalSignalKind::Attention,
    })
}

fn agent_from_protocol_name(value: &str) -> Option<AgentKind> {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized.contains("claude") {
        Some(AgentKind::Claude)
    } else if normalized.contains("codex") {
        Some(AgentKind::Codex)
    } else if normalized.contains("gemini") {
        Some(AgentKind::Gemini)
    } else {
        None
    }
}

impl FastTerminalView {
    pub fn init(cx: &mut App) {
        // gpui-component's root binds Tab to focus traversal. A terminal needs
        // the same keys to reach the PTY instead, so a more specific key
        // context shadows the root binding while this view owns focus.
        cx.bind_keys([
            KeyBinding::new("tab", SendTab, Some(TERMINAL_KEY_CONTEXT)),
            KeyBinding::new("shift-tab", SendBackTab, Some(TERMINAL_KEY_CONTEXT)),
        ]);
    }

    pub fn new<W, R>(
        writer: W,
        reader: R,
        config: TerminalConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self
    where
        W: Write + Send + 'static,
        R: Read + Send + 'static,
    {
        let (event_tx, event_rx) = flume::bounded(64);
        let mut terminal_options = AlacrittyConfig::default();
        terminal_options.scrolling_history = config.scrollback;
        let term = Arc::new(Mutex::new(Term::new(
            terminal_options,
            &TermSize {
                cols: config.cols,
                rows: config.rows,
            },
            FastEventProxy {
                sender: event_tx.clone(),
            },
        )));
        let renderer = TerminalRenderer::new(
            config.font_family.clone(),
            config.font_size,
            config.line_height_multiplier,
            config.colors.clone(),
        );
        let (repaint_tx, repaint_rx) = flume::bounded::<()>(1);
        let render_revision = Arc::new(AtomicU64::new(1));
        let parser_term = term.clone();
        let parser_revision = render_revision.clone();
        let protocol_event_tx = event_tx.clone();
        thread::Builder::new()
            .name("blackholes-pty-reader".into())
            .spawn(move || {
                let mut reader = reader;
                let mut parser: Processor = Processor::new();
                let mut osc_scanner = OscScanner::default();
                let mut buffer = [0_u8; 8 * 1024];
                loop {
                    match reader.read(&mut buffer) {
                        Ok(0) | Err(_) => break,
                        Ok(length) => {
                            osc_scanner.scan(&buffer[..length], |signal| {
                                let _ =
                                    protocol_event_tx.try_send(FastTerminalEvent::Agent(signal));
                            });
                            let mut term = parser_term.lock();
                            let was_alternate = term.mode().contains(TermMode::ALT_SCREEN);
                            parser.advance(&mut *term, &buffer[..length]);
                            let is_alternate = term.mode().contains(TermMode::ALT_SCREEN);
                            drop(term);
                            parser_revision.fetch_add(1, Ordering::Release);
                            if was_alternate != is_alternate {
                                let _ = protocol_event_tx
                                    .try_send(FastTerminalEvent::AlternateScreen(is_alternate));
                            }
                            let _ = repaint_tx.try_send(());
                        }
                    }
                }
                let _ = event_tx.try_send(FastTerminalEvent::Terminal(TerminalEvent::Exit));
                let _ = repaint_tx.try_send(());
            })
            .expect("PTY reader thread could not start");

        let reader_task = cx.spawn(async move |this: WeakEntity<Self>, cx| {
            while repaint_rx.recv_async().await.is_ok() {
                if this
                    .update(cx, |terminal, cx| {
                        terminal.process_events(cx);
                        terminal.record_output_activity(cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(8))
                    .await;
            }
        });

        let focus_handle = cx.focus_handle();
        let search_input = cx.new(|cx| InputState::new(window, cx).placeholder("Buscar"));
        let mut focus_subscriptions = vec![
            cx.observe_window_activation(window, |terminal, window, _| {
                terminal.report_focus(
                    window.is_window_active() && terminal.focus_handle.is_focused(window),
                );
            }),
            cx.on_focus(&focus_handle, window, |terminal, window, _| {
                terminal.report_focus(window.is_window_active());
            }),
            cx.on_blur(&focus_handle, window, |terminal, _, _| {
                terminal.report_focus(false);
            }),
        ];
        focus_subscriptions.push(cx.subscribe(
            &search_input,
            |terminal, _, event: &InputEvent, cx| match event {
                InputEvent::Change => terminal.refresh_search_results(cx),
                InputEvent::PressEnter { .. } => terminal.step_search_match(1, cx),
                InputEvent::Focus | InputEvent::Blur => {}
            },
        ));

        Self {
            term,
            renderer,
            renderer_measured: false,
            config,
            focus_handle,
            stdin_writer: Arc::new(Mutex::new(Box::new(writer))),
            event_rx,
            _reader_task: reader_task,
            _focus_subscriptions: focus_subscriptions,
            resize_callback: None,
            bell_callback: None,
            title_callback: None,
            exit_callback: None,
            clipboard_callback: None,
            agent_callback: None,
            screen_mode_callback: None,
            geometry: Arc::new(Mutex::new(TerminalGeometry::default())),
            paint_state: Arc::new(Mutex::new(TerminalPaintState::default())),
            render_revision,
            selecting: false,
            last_selection_point: None,
            ime_marked: None,
            search_input,
            search_open: false,
            search_regex: false,
            search_case_sensitive: false,
            search_pattern_valid: true,
            search_truncated: false,
            search_matches: Vec::new(),
            search_current: None,
            search_saved_selection: None,
            pending_mouse_report: None,
            agent_turn_pending: false,
            agent_output_seen: false,
            agent_settle_running: false,
            last_agent_output: None,
        }
    }

    pub fn with_resize_callback(
        mut self,
        callback: impl Fn(usize, usize) + Send + Sync + 'static,
    ) -> Self {
        self.resize_callback = Some(Arc::new(Box::new(callback)));
        self
    }

    pub fn with_bell_callback(mut self, callback: impl Fn(&mut Context<Self>) + 'static) -> Self {
        self.bell_callback = Some(Box::new(callback));
        self
    }

    pub fn with_title_callback(
        mut self,
        callback: impl Fn(&mut Context<Self>, &str) + 'static,
    ) -> Self {
        self.title_callback = Some(Box::new(callback));
        self
    }

    pub fn with_exit_callback(mut self, callback: impl Fn(&mut Context<Self>) + 'static) -> Self {
        self.exit_callback = Some(Box::new(callback));
        self
    }

    pub fn with_clipboard_store_callback(
        mut self,
        callback: impl Fn(&mut Context<Self>, &str) + 'static,
    ) -> Self {
        self.clipboard_callback = Some(Box::new(callback));
        self
    }

    pub fn with_agent_callback(
        mut self,
        callback: impl Fn(AgentTerminalSignal, &mut Context<Self>) + 'static,
    ) -> Self {
        self.agent_callback = Some(Box::new(callback));
        self
    }

    pub fn with_screen_mode_callback(
        mut self,
        callback: impl Fn(bool, &mut Context<Self>) + 'static,
    ) -> Self {
        self.screen_mode_callback = Some(Box::new(callback));
        self
    }

    pub fn focus_handle(&self) -> &FocusHandle {
        &self.focus_handle
    }

    pub fn update_config(&mut self, config: TerminalConfig, cx: &mut Context<Self>) {
        self.term
            .lock()
            .grid_mut()
            .update_history(config.scrollback);
        self.renderer = TerminalRenderer::new(
            config.font_family.clone(),
            config.font_size,
            config.line_height_multiplier,
            config.colors.clone(),
        );
        self.renderer_measured = false;
        self.paint_state.lock().clear();
        self.invalidate_terminal_frame();
        self.config = config;
        cx.notify();
    }

    fn write(&self, bytes: &[u8]) {
        let mut writer = self.stdin_writer.lock();
        let _ = writer.write_all(bytes);
        let _ = writer.flush();
    }

    fn invalidate_terminal_frame(&self) {
        self.render_revision.fetch_add(1, Ordering::Release);
    }

    fn scroll_to_bottom(&self) {
        self.term.lock().scroll_display(Scroll::Bottom);
        self.invalidate_terminal_frame();
    }

    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search_open {
            self.search_saved_selection = self.term.lock().selection.clone();
            self.search_open = true;
            self.refresh_search_results(cx);
        }
        self.focus_search_input(window, cx);
        cx.notify();
    }

    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.search_open {
            return;
        }
        self.search_open = false;
        self.search_matches.clear();
        self.search_current = None;
        self.search_truncated = false;
        self.term.lock().selection = self.search_saved_selection.take();
        self.invalidate_terminal_frame();
        window.focus(&self.focus_handle);
        cx.notify();
    }

    fn focus_search_input(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_input
            .update(cx, |input, cx| input.focus(window, cx));
    }

    fn refresh_search_results(&mut self, cx: &mut Context<Self>) {
        if !self.search_open {
            return;
        }

        let query = self.search_input.read(cx).value().to_string();
        let previous_start = self
            .search_current
            .and_then(|index| self.search_matches.get(index))
            .map(|search_match| search_match.start);
        let (matches, pattern_valid, truncated, visible_start, visible_end) = {
            let term = self.term.lock();
            collect_terminal_search_matches(
                &term,
                &query,
                self.search_regex,
                self.search_case_sensitive,
            )
        };

        self.search_pattern_valid = pattern_valid;
        self.search_truncated = truncated;
        self.search_matches = matches;
        self.search_current = previous_start
            .and_then(|start| {
                self.search_matches
                    .iter()
                    .position(|search_match| search_match.start == start)
            })
            .or_else(|| {
                self.search_matches.iter().position(|search_match| {
                    search_match.start.line >= visible_start
                        && search_match.start.line <= visible_end
                })
            })
            .or((!self.search_matches.is_empty()).then_some(0));
        self.activate_search_match();
        cx.notify();
    }

    fn step_search_match(&mut self, delta: i32, cx: &mut Context<Self>) {
        let count = self.search_matches.len();
        if count == 0 {
            return;
        }
        let current = self.search_current.unwrap_or(0);
        self.search_current = Some(if delta < 0 {
            current.checked_sub(1).unwrap_or(count - 1)
        } else {
            (current + 1) % count
        });
        self.activate_search_match();
        cx.notify();
    }

    fn activate_search_match(&mut self) {
        let active_match = self
            .search_current
            .and_then(|index| self.search_matches.get(index))
            .copied();
        let mut term = self.term.lock();
        if let Some(active_match) = active_match {
            let mut selection =
                Selection::new(SelectionType::Simple, active_match.start, Side::Left);
            selection.update(active_match.end, Side::Right);
            term.selection = Some(selection);
            term.scroll_to_point(active_match.start);
        } else {
            term.selection = self.search_saved_selection.clone();
        }
        drop(term);
        self.invalidate_terminal_frame();
    }

    fn search_count_label(&self) -> String {
        if !self.search_pattern_valid {
            return "Error".to_string();
        }
        let current = self.search_current.map_or(0, |index| index + 1);
        let suffix = if self.search_truncated { "+" } else { "" };
        format!("{current}/{}{suffix}", self.search_matches.len())
    }

    fn report_focus(&self, focused: bool) {
        if self.term.lock().mode().contains(TermMode::FOCUS_IN_OUT) {
            self.write(if focused { b"\x1b[I" } else { b"\x1b[O" });
        }
    }

    fn dispatch_agent_signal(&mut self, signal: AgentTerminalSignal, cx: &mut Context<Self>) {
        match signal.kind {
            AgentTerminalSignalKind::Started | AgentTerminalSignalKind::Working => {
                self.agent_turn_pending = false;
                self.agent_output_seen = false;
                self.last_agent_output = None;
            }
            AgentTerminalSignalKind::Attention => {
                self.agent_turn_pending = false;
                self.agent_output_seen = false;
                self.last_agent_output = None;
            }
        }
        if let Some(callback) = self.agent_callback.take() {
            callback(signal, cx);
            self.agent_callback = Some(callback);
        }
    }

    fn begin_agent_turn(&mut self, cx: &mut Context<Self>) {
        if !self.term.lock().mode().contains(TermMode::ALT_SCREEN) {
            return;
        }
        self.agent_turn_pending = true;
        self.agent_output_seen = false;
        self.last_agent_output = None;
        self.report_focus(false);
        if let Some(callback) = self.agent_callback.take() {
            callback(
                AgentTerminalSignal {
                    agent: None,
                    kind: AgentTerminalSignalKind::Working,
                },
                cx,
            );
            self.agent_callback = Some(callback);
        }
    }

    fn record_output_activity(&mut self, cx: &mut Context<Self>) {
        if !self.agent_turn_pending {
            return;
        }
        self.agent_output_seen = true;
        self.last_agent_output = Some(Instant::now());
        if self.agent_settle_running {
            return;
        }
        self.agent_settle_running = true;
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let mut wait = AGENT_OUTPUT_SETTLE_DELAY;
            loop {
                Timer::after(wait).await;
                let decision = this.update(cx, |terminal, cx| {
                    if !terminal.agent_turn_pending {
                        terminal.agent_settle_running = false;
                        return SettleDecision::Stop;
                    }
                    let Some(last_output) = terminal.last_agent_output else {
                        return SettleDecision::Wait(AGENT_OUTPUT_SETTLE_DELAY);
                    };
                    let elapsed = last_output.elapsed();
                    if !terminal.agent_output_seen || elapsed < AGENT_OUTPUT_SETTLE_DELAY {
                        return SettleDecision::Wait(
                            AGENT_OUTPUT_SETTLE_DELAY.saturating_sub(elapsed),
                        );
                    }

                    terminal.agent_settle_running = false;
                    terminal.dispatch_agent_signal(
                        AgentTerminalSignal {
                            agent: None,
                            kind: AgentTerminalSignalKind::Attention,
                        },
                        cx,
                    );
                    SettleDecision::Stop
                });
                match decision {
                    Ok(SettleDecision::Wait(duration)) => wait = duration,
                    Ok(SettleDecision::Stop) | Err(_) => return,
                }
            }
        })
        .detach();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        log_keystroke(event);
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;
        if modifiers.secondary()
            && !modifiers.alt
            && !modifiers.function
            && key.eq_ignore_ascii_case("f")
        {
            self.open_search(window, cx);
            cx.stop_propagation();
            return;
        }

        let search_input_focused =
            self.search_open && self.search_input.focus_handle(cx).is_focused(window);
        if search_input_focused {
            match key {
                "escape" => self.close_search(window, cx),
                "enter" if modifiers.shift => self.step_search_match(-1, cx),
                "up" => self.step_search_match(-1, cx),
                "down" => self.step_search_match(1, cx),
                _ if modifiers.secondary() && key.eq_ignore_ascii_case("g") => {
                    self.step_search_match(if modifiers.shift { -1 } else { 1 }, cx)
                }
                // All remaining keystrokes belong to the search input. Let
                // its own key bindings and native text input handle them,
                // without leaking anything into the terminal PTY.
                _ => return,
            }
            cx.stop_propagation();
            return;
        }

        if event.keystroke.modifiers.platform && key == "enter" {
            // Cmd+Return is intentionally left unassigned.
            return;
        }
        if event.keystroke.modifiers.shift && key == "enter" {
            self.report_focus(true);
            self.scroll_to_bottom();
            // Encode Shift+Return as Alt+Return (Escape + Return), the
            // traditional terminal shortcut Codex and Claude use for
            // multiline input without submitting the prompt.
            self.write(b"\x1b\r");
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if event.keystroke.modifiers.platform && key.eq_ignore_ascii_case("c") {
            self.report_focus(true);
            if let Some(text) = self.term.lock().selection_to_string() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            cx.stop_propagation();
            return;
        }
        if event.keystroke.modifiers.platform && key.eq_ignore_ascii_case("v") {
            self.report_focus(true);
            let Some(item) = cx.read_from_clipboard() else {
                return;
            };
            if let Some(text) = item.text() {
                let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
                if bracketed {
                    self.write(b"\x1b[200~");
                }
                self.write(text.replace('\r', "").as_bytes());
                if bracketed {
                    self.write(b"\x1b[201~");
                }
            } else {
                // Codex and Claude Code use Ctrl+V to read an image from the
                // native clipboard and attach it to the current prompt.
                self.write(b"\x16");
            }
            cx.stop_propagation();
            return;
        }

        // On macOS, a plain Space key is inconsistently committed through
        // NSTextInputContext after the terminal regains focus. Send it
        // directly to the PTY unless an IME composition is active; in that
        // case Space must remain available for candidate selection.
        #[cfg(target_os = "macos")]
        if key == "space"
            && self.ime_marked.is_none()
            && !modifiers.alt
            && !modifiers.control
            && !modifiers.platform
            && !modifiers.function
        {
            self.report_focus(true);
            self.scroll_to_bottom();
            self.write(b" ");
            cx.stop_propagation();
            cx.notify();
            return;
        }

        // Printable macOS keystrokes must reach NSTextInputContext. It owns
        // the active keyboard layout, Shift/Caps Lock resolution, dead-key
        // state (Option+E, Option+N, ...), and multi-stage IME composition.
        // The committed result comes back through `replace_text_in_range`.
        // Consuming these here would bypass AppKit and turn Option+E into
        // Meta+E while also losing some shifted characters.
        #[cfg(target_os = "macos")]
        if event.keystroke.key_char.is_some()
            && !matches!(key, "enter" | "tab")
            && !event.keystroke.modifiers.control
            && !event.keystroke.modifiers.platform
            && !event.keystroke.modifiers.function
        {
            return;
        }

        self.report_focus(true);
        self.scroll_to_bottom();
        let mode = *self.term.lock().mode();
        if let Some(bytes) = keystroke_to_bytes(&event.keystroke, mode) {
            self.write(&bytes);
            // Claim the keystroke. Without this GPUI also forwards it to the
            // macOS input context, which inserts the same character a second
            // time through `replace_text_in_range` -- every letter arrives at
            // the shell doubled.
            cx.stop_propagation();
            if key == "enter" && !event.keystroke.modifiers.modified() {
                self.begin_agent_turn(cx);
            }
        }
        cx.notify();
    }

    fn on_action_tab(&mut self, _: &SendTab, _: &mut Window, cx: &mut Context<Self>) {
        self.send_tab(false, cx);
    }

    fn on_action_back_tab(&mut self, _: &SendBackTab, _: &mut Window, cx: &mut Context<Self>) {
        self.send_tab(true, cx);
    }

    fn send_tab(&mut self, reverse: bool, cx: &mut Context<Self>) {
        self.report_focus(true);
        self.scroll_to_bottom();
        self.write(if reverse { b"\x1b[Z" } else { b"\t" });
        cx.notify();
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        self.report_focus(true);
        let point = self.point_for_position(event.position);
        if event.modifiers.platform {
            if let Some(url) = self.url_at_point(point) {
                cx.open_url(&url);
                return;
            }
        }
        let mode = *self.term.lock().mode();
        let modifiers = encode_modifiers(
            event.modifiers.shift,
            event.modifiers.alt,
            event.modifiers.control,
        );
        // When a TUI has grabbed the mouse, dragging used to be impossible:
        // the press went straight to the application and no selection was
        // ever started. Hold single clicks back instead -- if the pointer
        // moves we treat it as a selection, and if it does not we forward the
        // press on release so TUI buttons still work. Shift always forces a
        // local selection, and a double click is already an explicit one.
        self.pending_mouse_report = None;
        if !event.modifiers.shift
            && event.click_count == 1
            && let Some(bytes) = mouse_button_report(event.button, true, point, modifiers, mode)
        {
            self.pending_mouse_report = Some((event.button, bytes));
        }
        let selection_type = match event.click_count {
            2 => SelectionType::Semantic,
            3.. => SelectionType::Lines,
            _ if event.modifiers.alt => SelectionType::Block,
            _ => SelectionType::Simple,
        };
        self.term.lock().selection = Some(Selection::new(selection_type, point, Side::Left));
        self.invalidate_terminal_frame();
        self.selecting = true;
        self.last_selection_point = Some(point);
        cx.notify();
    }

    fn url_at_point(&self, point: TerminalPoint) -> Option<String> {
        static URL: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r#"https?://[^\s<>\[\]{}\"']+"#).expect("valid URL pattern")
        });
        {
            let term = self.term.lock();
            let line = term.bounds_to_string(
                TerminalPoint::new(point.line, Column(0)),
                TerminalPoint::new(point.line, term.last_column()),
            );
            URL.find_iter(&line)
                .find(|found| found.start() <= point.column.0 && point.column.0 < found.end())
                .map(|found| {
                    found
                        .as_str()
                        .trim_end_matches(['.', ',', ';', ':', '!', '?', ')'])
                        .to_string()
                })
        }
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !event.dragging() {
            return;
        }
        self.extend_selection(event.position, cx);
    }

    /// Grows the active selection to `position`, repainting only when the
    /// pointer actually crossed into another cell. A drag reports far more
    /// moves than there are cells, and every repaint redraws the whole grid.
    fn extend_selection(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        let point = self.point_for_position(position);
        if self.last_selection_point == Some(point) {
            return;
        }
        self.last_selection_point = Some(point);
        // The pointer crossed a cell, so this is a drag: the click belongs to
        // the selection, not to the application underneath.
        self.pending_mouse_report = None;
        if let Some(selection) = &mut self.term.lock().selection {
            selection.update(point, Side::Right);
        }
        self.invalidate_terminal_frame();
        cx.notify();
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.release_mouse(event.position, event.button, event.modifiers, cx);
    }

    /// Idempotent so the window-level listener and the element listener can
    /// both call it for the same release.
    fn release_mouse(
        &mut self,
        position: Point<Pixels>,
        button: MouseButton,
        modifiers: gpui::Modifiers,
        cx: &mut Context<Self>,
    ) {
        let point = self.point_for_position(position);
        let mode = *self.term.lock().mode();
        let encoded = encode_modifiers(modifiers.shift, modifiers.alt, modifiers.control);
        if let Some((pressed, down_report)) = self.pending_mouse_report.take() {
            // Never moved: it was a click for the application, not a
            // selection. Send the press we held back, then the release.
            self.term.lock().selection = None;
            self.invalidate_terminal_frame();
            self.write(&down_report);
            if let Some(bytes) = mouse_button_report(pressed, false, point, encoded, mode) {
                self.write(&bytes);
            }
            self.finish_selection(cx);
            return;
        }
        if !self.selecting {
            if let Some(bytes) = mouse_button_report(button, false, point, encoded, mode) {
                self.write(&bytes);
            }
            return;
        }
        if let Some(selection) = &mut self.term.lock().selection {
            selection.update(point, Side::Right);
        }
        self.invalidate_terminal_frame();
        self.finish_selection(cx);
    }

    fn finish_selection(&mut self, cx: &mut Context<Self>) {
        if !self.selecting {
            return;
        }
        self.selecting = false;
        self.last_selection_point = None;
        cx.notify();
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let geometry = *self.geometry.lock();
        let pixel_delta = event.delta.pixel_delta(geometry.cell_height).y;
        let pixels: f32 = pixel_delta.into();
        let cell_height: f32 = geometry.cell_height.into();
        let lines = (pixels / cell_height).round() as i32;
        let lines = if lines == 0 {
            pixels.signum() as i32
        } else {
            lines
        }
        .clamp(-12, 12);
        if lines == 0 {
            return;
        }
        let point = self.point_for_position(event.position);
        let mode = *self.term.lock().mode();
        let modifiers = encode_modifiers(
            event.modifiers.shift,
            event.modifiers.alt,
            event.modifiers.control,
        );
        if let Some(bytes) = scroll_report(lines, point, modifiers, mode) {
            self.write(&bytes);
        } else {
            self.term.lock().scroll_display(Scroll::Delta(lines));
            self.invalidate_terminal_frame();
            cx.notify();
        }
    }

    fn point_for_position(&self, position: Point<Pixels>) -> TerminalPoint {
        let geometry = *self.geometry.lock();
        let x: f32 = (position.x - geometry.bounds.origin.x - self.config.padding.left).into();
        let y: f32 = (position.y - geometry.bounds.origin.y - self.config.padding.top).into();
        let cell_width: f32 = geometry.cell_width.into();
        let cell_height: f32 = geometry.cell_height.into();
        let column = ((x / cell_width).floor() as isize).max(0) as usize;
        let visible_line = ((y / cell_height).floor() as i32).max(0);
        let term = self.term.lock();
        let offset = term.grid().display_offset() as i32;
        let columns = term.columns();
        let rows = term.screen_lines();
        TerminalPoint::new(
            Line((visible_line.min(rows.saturating_sub(1) as i32)) - offset),
            Column(column.min(columns.saturating_sub(1))),
        )
    }

    fn process_events(&mut self, cx: &mut Context<Self>) {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                FastTerminalEvent::Agent(signal) => self.dispatch_agent_signal(signal, cx),
                FastTerminalEvent::AlternateScreen(alternate) => {
                    if !alternate {
                        self.agent_turn_pending = false;
                        self.agent_output_seen = false;
                        self.last_agent_output = None;
                    }
                    if let Some(callback) = self.screen_mode_callback.take() {
                        callback(alternate, cx);
                        self.screen_mode_callback = Some(callback);
                    }
                }
                FastTerminalEvent::Terminal(event) => match event {
                    TerminalEvent::Bell => {
                        self.agent_turn_pending = false;
                        self.agent_output_seen = false;
                        self.last_agent_output = None;
                        if let Some(callback) = self.bell_callback.take() {
                            callback(cx);
                            self.bell_callback = Some(callback);
                        }
                    }
                    TerminalEvent::Title(title) => {
                        if let Some(callback) = self.title_callback.take() {
                            callback(cx, &title);
                            self.title_callback = Some(callback);
                        }
                    }
                    TerminalEvent::ClipboardStore(text) => {
                        if let Some(callback) = self.clipboard_callback.take() {
                            callback(cx, &text);
                            self.clipboard_callback = Some(callback);
                        }
                    }
                    TerminalEvent::Exit => {
                        self.agent_turn_pending = false;
                        if let Some(callback) = self.exit_callback.take() {
                            callback(cx);
                            self.exit_callback = Some(callback);
                        }
                    }
                    TerminalEvent::Wakeup | TerminalEvent::ClipboardLoad => {}
                },
            }
        }
    }
}

fn collect_terminal_search_matches(
    term: &Term<FastEventProxy>,
    query: &str,
    regex_enabled: bool,
    case_sensitive: bool,
) -> (Vec<TerminalSearchMatch>, bool, bool, Line, Line) {
    let visible_start = Line(-(term.grid().display_offset() as i32));
    let visible_end = Line(visible_start.0 + term.screen_lines().saturating_sub(1) as i32);
    if query.is_empty() {
        return (Vec::new(), true, false, visible_start, visible_end);
    }

    let source = if regex_enabled {
        query.to_string()
    } else {
        regex::escape(query)
    };
    let pattern = if case_sensitive {
        format!("(?-i:{source})")
    } else {
        format!("(?i:{source})")
    };
    let Ok(mut regex) = RegexSearch::new(&pattern) else {
        return (Vec::new(), false, false, visible_start, visible_end);
    };

    let last_column = term.last_column();
    let bottommost_line = term.bottommost_line();
    let search_end = TerminalPoint::new(bottommost_line, last_column);
    let mut origin = TerminalPoint::new(term.topmost_line(), Column(0));
    let mut matches = Vec::new();
    let mut truncated = false;

    loop {
        let Some(regex_match) = term.regex_search_right(&mut regex, origin, search_end) else {
            break;
        };
        let start = *regex_match.start();
        let end = *regex_match.end();
        matches.push(TerminalSearchMatch { start, end });
        if matches.len() == MAX_TERMINAL_SEARCH_MATCHES {
            truncated = true;
            break;
        }

        origin = if end.column < last_column {
            TerminalPoint::new(end.line, Column(end.column.0 + 1))
        } else if end.line < bottommost_line {
            TerminalPoint::new(Line(end.line.0 + 1), Column(0))
        } else {
            break;
        };
    }

    (matches, true, truncated, visible_start, visible_end)
}

/// Diagnostic trace of what GPUI actually delivers, enabled by setting
/// `BLACKHOLES_KEY_LOG` to a file path. Off by default and costs one
/// `env::var` lookup per keystroke when unset.
fn log_keystroke(event: &KeyDownEvent) {
    static KEY_LOG: LazyLock<Option<std::path::PathBuf>> =
        LazyLock::new(|| std::env::var_os("BLACKHOLES_KEY_LOG").map(std::path::PathBuf::from));
    let Some(path) = KEY_LOG.as_ref() else {
        return;
    };
    let modifiers = &event.keystroke.modifiers;
    let line = format!(
        "key={:?} key_char={:?} shift={} alt={} ctrl={} cmd={} fn={} held={}\n",
        event.keystroke.key,
        event.keystroke.key_char,
        modifiers.shift,
        modifiers.alt,
        modifiers.control,
        modifiers.platform,
        modifiers.function,
        event.is_held,
    );
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// A terminal has no editable document to offer the input method: everything
/// typed goes straight to the PTY and the shell owns the line buffer. These
/// methods therefore report an empty document and exist mainly so macOS
/// accepts the view as an input client and delivers composed text through
/// `replace_text_in_range`.
impl EntityInputHandler for FastTerminalView {
    fn text_for_range(
        &mut self,
        _range: Range<usize>,
        _adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        // Reporting an empty selection at the origin (rather than `None`) is
        // what keeps the IME willing to compose into this view.
        Some(UTF16Selection {
            range: 0..0,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        let marked = self.ime_marked.as_ref()?;
        Some(0..marked.encode_utf16().count())
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.ime_marked = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ime_marked = None;
        if text.is_empty() {
            return;
        }
        self.report_focus(true);
        self.scroll_to_bottom();
        let bracketed = self.term.lock().mode().contains(TermMode::BRACKETED_PASTE);
        // Committed IME text can be several characters at once (a CJK phrase).
        // Bracket it like a paste so shells and TUIs treat it atomically.
        if bracketed && text.chars().count() > 1 {
            self.write(b"\x1b[200~");
            self.write(text.as_bytes());
            self.write(b"\x1b[201~");
        } else {
            self.write(text.as_bytes());
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.ime_marked = if new_text.is_empty() {
            None
        } else {
            Some(new_text.to_string())
        };
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // Anchor the candidate window on the cursor cell so it does not cover
        // what is being typed.
        let geometry = *self.geometry.lock();
        let term = self.term.lock();
        let cursor = term.grid().cursor.point;
        let display_offset = term.grid().display_offset() as i32;
        let line = (cursor.line.0 + display_offset).max(0) as f32;
        drop(term);
        let origin = geometry.bounds.origin
            + Point::new(
                self.config.padding.left + geometry.cell_width * cursor.column.0 as f32,
                self.config.padding.top + geometry.cell_height * line,
            );
        if !element_bounds.contains(&origin) {
            return Some(element_bounds);
        }
        Some(Bounds {
            origin,
            size: size(geometry.cell_width, geometry.cell_height),
        })
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl Render for FastTerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.process_events(cx);
        if !self.renderer_measured {
            self.renderer.measure_cell(window);
            self.renderer_measured = true;
        }
        let search_bar = self.search_open.then(|| {
            let has_matches = !self.search_matches.is_empty();
            h_flex()
                .id(("terminal-search-bar", cx.entity_id()))
                .absolute()
                .top(px(14.))
                .left(px(18.))
                .right(px(18.))
                .h(px(50.))
                .px_2()
                .gap_1()
                .rounded(px(9.))
                .border_1()
                .border_color(rgb(0x30434c))
                .bg(rgb(0x101b21))
                .shadow_lg()
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_key_down(cx.listener(Self::on_key_down))
                .child(
                    div().flex_1().min_w(px(160.)).child(
                        Input::new(&self.search_input)
                            .appearance(false)
                            .bordered(false)
                            .focus_bordered(false)
                            .small()
                            .w_full(),
                    ),
                )
                .child(
                    Button::new(("terminal-search-regex", cx.entity_id()))
                        .label(".*")
                        .selected(self.search_regex)
                        .tooltip("Expresión regular")
                        .ghost()
                        .xsmall()
                        .compact()
                        .tab_stop(false)
                        .on_click(cx.listener(|terminal, _, window, cx| {
                            terminal.search_regex = !terminal.search_regex;
                            terminal.refresh_search_results(cx);
                            terminal.focus_search_input(window, cx);
                        })),
                )
                .child(
                    Button::new(("terminal-search-case", cx.entity_id()))
                        .label("Aa")
                        .selected(self.search_case_sensitive)
                        .tooltip("Distinguir mayúsculas")
                        .ghost()
                        .xsmall()
                        .compact()
                        .tab_stop(false)
                        .on_click(cx.listener(|terminal, _, window, cx| {
                            terminal.search_case_sensitive = !terminal.search_case_sensitive;
                            terminal.refresh_search_results(cx);
                            terminal.focus_search_input(window, cx);
                        })),
                )
                .child(
                    div()
                        .min_w(px(52.))
                        .text_sm()
                        .text_center()
                        .text_color(if self.search_pattern_valid {
                            rgb(0x8f9aa8)
                        } else {
                            rgb(0xff7b72)
                        })
                        .child(self.search_count_label()),
                )
                .child(
                    Button::new(("terminal-search-next", cx.entity_id()))
                        .icon(IconName::ArrowDown)
                        .tooltip("Siguiente coincidencia")
                        .disabled(!has_matches)
                        .ghost()
                        .xsmall()
                        .compact()
                        .tab_stop(false)
                        .on_click(cx.listener(|terminal, _, window, cx| {
                            terminal.step_search_match(1, cx);
                            terminal.focus_search_input(window, cx);
                        })),
                )
                .child(
                    Button::new(("terminal-search-previous", cx.entity_id()))
                        .icon(IconName::ArrowUp)
                        .tooltip("Coincidencia anterior")
                        .disabled(!has_matches)
                        .ghost()
                        .xsmall()
                        .compact()
                        .tab_stop(false)
                        .on_click(cx.listener(|terminal, _, window, cx| {
                            terminal.step_search_match(-1, cx);
                            terminal.focus_search_input(window, cx);
                        })),
                )
                .child(
                    Button::new(("terminal-search-close", cx.entity_id()))
                        .icon(IconName::Close)
                        .tooltip("Cerrar búsqueda")
                        .ghost()
                        .xsmall()
                        .compact()
                        .tab_stop(false)
                        .on_click(cx.listener(|terminal, _, window, cx| {
                            terminal.close_search(window, cx);
                        })),
                )
        });
        let term = self.term.clone();
        let renderer = self.renderer.clone();
        let resize_callback = self.resize_callback.clone();
        let padding = self.config.padding;
        let geometry = self.geometry.clone();
        let paint_state = self.paint_state.clone();
        let render_revision = self.render_revision.clone();
        let background = self.config.colors.clone();
        let input_entity = cx.entity();
        let input_focus = self.focus_handle.clone();
        div()
            .relative()
            .size_full()
            .bg(background.resolve(Color::Named(NamedColor::Background), &Default::default()))
            .key_context(TERMINAL_KEY_CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::on_action_tab))
            .on_action(cx.listener(Self::on_action_back_tab))
            .on_key_down(cx.listener(Self::on_key_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .child(
                canvas(
                    move |bounds, _, _| bounds,
                    move |bounds, _, window, cx| {
                        // Registering an input handler is what makes macOS
                        // route composed text (dead keys, CJK) to this view
                        // instead of dropping it.
                        window.handle_input(
                            &input_focus,
                            ElementInputHandler::new(bounds, input_entity.clone()),
                            cx,
                        );
                        // The div's own mouse listeners only fire while the
                        // pointer is inside it, so a drag that runs past the
                        // edge would freeze the selection and leave
                        // `selecting` stuck on. Follow the drag at the window
                        // level instead; both handlers bail immediately when
                        // no selection is in flight.
                        let drag_entity = input_entity.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _window, cx| {
                            if phase != DispatchPhase::Bubble || !event.dragging() {
                                return;
                            }
                            drag_entity.update(cx, |view, cx| {
                                view.extend_selection(event.position, cx);
                            });
                        });
                        let release_entity = input_entity.clone();
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, _window, cx| {
                            if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                                return;
                            }
                            release_entity.update(cx, |view, cx| {
                                view.extend_selection(event.position, cx);
                                view.release_mouse(
                                    event.position,
                                    event.button,
                                    event.modifiers,
                                    cx,
                                );
                            });
                        });
                        *geometry.lock() = TerminalGeometry {
                            bounds,
                            cell_width: renderer.cell_width,
                            cell_height: renderer.cell_height,
                        };
                        let available_width: f32 =
                            (bounds.size.width - padding.left - padding.right).into();
                        let available_height: f32 =
                            (bounds.size.height - padding.top - padding.bottom).into();
                        let cell_width: f32 = renderer.cell_width.into();
                        let cell_height: f32 = renderer.cell_height.into();
                        let cols = ((available_width / cell_width) as usize).max(1);
                        let rows = ((available_height / cell_height) as usize).max(1);
                        let revision = render_revision.load(Ordering::Acquire);
                        let needs_capture = {
                            let paint_state = paint_state.lock();
                            paint_state.revision != revision
                                || paint_state.frame.as_ref().is_none_or(|frame| {
                                    frame.columns != cols || frame.rows.len() != rows
                                })
                        };
                        let captured_frame = if needs_capture {
                            term.try_lock().map(|mut term| {
                                if cols != term.columns() || rows != term.screen_lines() {
                                    if let Some(callback) = &resize_callback {
                                        callback(cols, rows);
                                    }
                                    term.resize(TermSize { cols, rows });
                                }
                                Arc::new(TerminalFrame::capture(&renderer, &term))
                            })
                        } else {
                            None
                        };

                        // Warp keeps terminal mutation and GPU painting as two
                        // separate phases. Retaining the terminal lock while
                        // shaping and issuing draw calls stalls the PTY parser
                        // under sustained output. Snapshot quickly instead;
                        // if parsing owns the lock, paint the last complete
                        // frame and consume the new one on the next coalesced
                        // repaint.
                        let mut paint_state = paint_state.lock();
                        if let Some(frame) = captured_frame {
                            paint_state.frame = Some(frame);
                            paint_state.revision = revision;
                        }
                        if let Some(frame) = paint_state.frame.clone() {
                            paint_terminal(
                                &renderer,
                                bounds,
                                padding,
                                &frame,
                                &mut paint_state.rows,
                                window,
                                cx,
                            );
                        }
                    },
                )
                .size_full(),
            )
            .children(search_bar)
    }
}

struct TermSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

#[derive(Default)]
struct TerminalPaintState {
    frame: Option<Arc<TerminalFrame>>,
    rows: Vec<CachedPaintRow>,
    revision: u64,
}

impl TerminalPaintState {
    fn clear(&mut self) {
        self.frame = None;
        self.rows.clear();
        self.revision = 0;
    }
}

struct TerminalFrame {
    default_background: Hsla,
    columns: usize,
    rows: Vec<Vec<PaintCell>>,
    cursor: Option<PaintCursor>,
}

impl TerminalFrame {
    /// Copies the visible grid while the parser lock is held. GPU painting and
    /// text shaping consume this immutable snapshot after the lock is released.
    fn capture(renderer: &TerminalRenderer, term: &Term<FastEventProxy>) -> Self {
        let content = term.renderable_content();
        let colors = content.colors;
        let default_background = renderer
            .palette
            .resolve(Color::Named(NamedColor::Background), colors);
        let default_foreground = renderer
            .palette
            .resolve(Color::Named(NamedColor::Foreground), colors);
        let display_offset = content.display_offset as i32;
        let selection = content.selection;
        let mut rows = (0..term.screen_lines())
            .map(|_| Vec::with_capacity(term.columns()))
            .collect::<Vec<_>>();

        for indexed in content.display_iter {
            let row = indexed.point.line.0 + display_offset;
            if row < 0 || row >= term.screen_lines() as i32 {
                continue;
            }
            let selected = selection.is_some_and(|selection| selection.contains(indexed.point));
            let mut foreground = renderer.palette.resolve(indexed.cell.fg, colors);
            let mut background = renderer.palette.resolve(indexed.cell.bg, colors);
            if indexed.cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut foreground, &mut background);
            }
            if indexed.cell.flags.contains(Flags::DIM) && !selected {
                foreground = foreground.opacity(0.66);
            }
            if selected {
                foreground = default_background;
                background = default_foreground;
            }
            rows[row as usize].push(PaintCell {
                column: indexed.point.column.0,
                character: indexed.cell.c,
                zerowidth: indexed.cell.zerowidth().map(Arc::from),
                foreground,
                background,
                flags: indexed.cell.flags,
                selected,
            });
        }

        let cursor = (display_offset == 0
            && content.cursor.shape != CursorShape::Hidden
            && content.mode.contains(TermMode::SHOW_CURSOR))
        .then(|| PaintCursor {
            row: content.cursor.point.line.0.max(0) as usize,
            column: content.cursor.point.column.0,
            shape: content.cursor.shape,
            color: renderer
                .palette
                .resolve(Color::Named(NamedColor::Cursor), colors),
        });
        if let Some(cursor) = &cursor
            && cursor.shape == CursorShape::Block
            && let Some(cell) = rows
                .get_mut(cursor.row)
                .and_then(|row| row.iter_mut().find(|cell| cell.column == cursor.column))
        {
            // Warp paints a block cursor behind its glyph. Keep that glyph
            // visible using the terminal background as its contrast color.
            cell.foreground = default_background;
        }

        Self {
            default_background,
            columns: term.columns(),
            rows,
            cursor,
        }
    }
}

#[derive(Clone, PartialEq)]
struct PaintCell {
    column: usize,
    character: char,
    zerowidth: Option<Arc<[char]>>,
    foreground: Hsla,
    background: Hsla,
    flags: Flags,
    selected: bool,
}

struct PaintCursor {
    row: usize,
    column: usize,
    shape: CursorShape,
    color: Hsla,
}

#[derive(Default)]
struct CachedPaintRow {
    source: Vec<PaintCell>,
    text_batches: Vec<CachedTextBatch>,
}

struct CachedTextBatch {
    start_column: usize,
    line: ShapedLine,
}

fn paint_terminal(
    renderer: &TerminalRenderer,
    bounds: Bounds<Pixels>,
    padding: Edges<Pixels>,
    frame: &TerminalFrame,
    cached_rows: &mut Vec<CachedPaintRow>,
    window: &mut Window,
    cx: &mut App,
) {
    window.paint_quad(quad(
        bounds,
        px(0.),
        frame.default_background,
        Edges::default(),
        transparent_black(),
        Default::default(),
    ));
    let origin = Point {
        x: bounds.origin.x + padding.left,
        y: bounds.origin.y + padding.top,
    };

    paint_backgrounds(
        &frame.rows,
        frame.default_background,
        renderer,
        origin,
        window,
    );
    if let Some(cursor) = &frame.cursor {
        paint_cursor(cursor, renderer, origin, window);
    }
    paint_text(&frame.rows, cached_rows, renderer, origin, window, cx);
}

struct BackgroundBatch {
    row: usize,
    start_column: usize,
    end_column: usize,
    color: Hsla,
}

fn paint_backgrounds(
    rows: &[Vec<PaintCell>],
    default_background: Hsla,
    renderer: &TerminalRenderer,
    origin: Point<Pixels>,
    window: &mut Window,
) {
    let mut batch: Option<BackgroundBatch> = None;
    for (row, cells) in rows.iter().enumerate() {
        for cell in cells {
            if cell.background == default_background && !cell.selected {
                paint_background_batch(batch.take(), renderer, origin, window);
                continue;
            }
            match &mut batch {
                Some(batch)
                    if batch.row == row
                        && batch.end_column == cell.column
                        && batch.color == cell.background =>
                {
                    batch.end_column += 1;
                }
                _ => {
                    paint_background_batch(batch.take(), renderer, origin, window);
                    batch = Some(BackgroundBatch {
                        row,
                        start_column: cell.column,
                        end_column: cell.column + 1,
                        color: cell.background,
                    });
                }
            }
        }
        paint_background_batch(batch.take(), renderer, origin, window);
    }
}

fn paint_background_batch(
    batch: Option<BackgroundBatch>,
    renderer: &TerminalRenderer,
    origin: Point<Pixels>,
    window: &mut Window,
) {
    let Some(batch) = batch else {
        return;
    };
    window.paint_quad(quad(
        Bounds {
            origin: Point {
                x: origin.x + renderer.cell_width * batch.start_column as f32,
                y: origin.y + renderer.cell_height * batch.row as f32,
            },
            size: Size {
                width: renderer.cell_width * (batch.end_column - batch.start_column) as f32,
                height: renderer.cell_height,
            },
        },
        px(0.),
        batch.color,
        Edges::default(),
        transparent_black(),
        Default::default(),
    ));
}

struct TextBatch {
    start_column: usize,
    next_column: usize,
    text: String,
    foreground: Hsla,
    bold: bool,
    italic: bool,
    underline: bool,
    undercurl: bool,
    strikethrough: bool,
}

fn paint_text(
    rows: &[Vec<PaintCell>],
    cached_rows: &mut Vec<CachedPaintRow>,
    renderer: &TerminalRenderer,
    origin: Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    cached_rows.resize_with(rows.len(), CachedPaintRow::default);
    cached_rows.truncate(rows.len());

    for (row, cells) in rows.iter().enumerate() {
        let cached = &mut cached_rows[row];
        if cached.source.as_slice() != cells.as_slice() {
            cached.text_batches = shape_text_batches(cells, renderer, window);
            cached.source.clone_from(cells);
        }

        for cell in cells {
            if should_paint_native_glyph(cell) {
                paint_native_glyph(cell, row, renderer, origin, window);
            }
        }

        let base_height = renderer.cell_height / renderer.line_height_multiplier;
        let y = origin.y
            + renderer.cell_height * row as f32
            + (renderer.cell_height - base_height) / 2.;
        for batch in &cached.text_batches {
            let _ = batch.line.paint(
                Point {
                    x: origin.x + renderer.cell_width * batch.start_column as f32,
                    y,
                },
                renderer.cell_height,
                window,
                cx,
            );
        }
    }
}

fn shape_text_batches(
    cells: &[PaintCell],
    renderer: &TerminalRenderer,
    window: &mut Window,
) -> Vec<CachedTextBatch> {
    let mut shaped = Vec::new();
    let mut batch: Option<TextBatch> = None;
    for cell in cells {
        if cell
            .flags
            .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            || cell.flags.contains(Flags::HIDDEN)
            || should_paint_native_glyph(cell)
        {
            shape_text_batch(batch.take(), renderer, window, &mut shaped);
            continue;
        }
        let character = if cell.character == '\0' {
            ' '
        } else {
            cell.character
        };
        let bold = cell.flags.contains(Flags::BOLD);
        let italic = cell.flags.contains(Flags::ITALIC);
        let underline = cell.flags.intersects(
            Flags::UNDERLINE
                | Flags::DOUBLE_UNDERLINE
                | Flags::DOTTED_UNDERLINE
                | Flags::DASHED_UNDERLINE,
        );
        let undercurl = cell.flags.contains(Flags::UNDERCURL);
        let strikethrough = cell.flags.contains(Flags::STRIKEOUT);
        let wide = cell.flags.contains(Flags::WIDE_CHAR);

        if character == ' ' && cell.zerowidth.is_none() && batch.is_none() {
            continue;
        }
        match &mut batch {
            Some(batch)
                if !wide
                    && batch.next_column == cell.column
                    && batch.foreground == cell.foreground
                    && batch.bold == bold
                    && batch.italic == italic
                    && batch.underline == underline
                    && batch.undercurl == undercurl
                    && batch.strikethrough == strikethrough =>
            {
                batch.text.push(character);
                append_zerowidth(&mut batch.text, cell.zerowidth.as_deref());
                batch.next_column += 1;
            }
            _ => {
                shape_text_batch(batch.take(), renderer, window, &mut shaped);
                if character != ' ' || cell.zerowidth.is_some() {
                    let mut text = character.to_string();
                    append_zerowidth(&mut text, cell.zerowidth.as_deref());
                    batch = Some(TextBatch {
                        start_column: cell.column,
                        next_column: cell.column + 1,
                        text,
                        foreground: cell.foreground,
                        bold,
                        italic,
                        underline,
                        undercurl,
                        strikethrough,
                    });
                }
            }
        }
        if wide {
            shape_text_batch(batch.take(), renderer, window, &mut shaped);
        }
    }
    shape_text_batch(batch, renderer, window, &mut shaped);
    shaped
}

fn append_zerowidth(text: &mut String, zerowidth: Option<&[char]>) {
    if let Some(characters) = zerowidth {
        text.extend(characters);
    }
}

fn shape_text_batch(
    batch: Option<TextBatch>,
    renderer: &TerminalRenderer,
    window: &mut Window,
    shaped: &mut Vec<CachedTextBatch>,
) {
    let Some(batch) = batch else {
        return;
    };
    let font = Font {
        family: renderer.font_family.clone().into(),
        features: FontFeatures::disable_ligatures(),
        fallbacks: None,
        weight: if batch.bold {
            FontWeight::BOLD
        } else {
            FontWeight::NORMAL
        },
        style: if batch.italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        },
    };
    let run = TextRun {
        len: batch.text.len(),
        font,
        color: batch.foreground,
        background_color: None,
        underline: (batch.underline || batch.undercurl).then_some(UnderlineStyle {
            thickness: px(1.),
            color: Some(batch.foreground),
            wavy: batch.undercurl,
        }),
        strikethrough: batch.strikethrough.then_some(StrikethroughStyle {
            thickness: px(1.),
            color: Some(batch.foreground),
        }),
    };
    let line = window.text_system().shape_line(
        SharedString::from(batch.text),
        renderer.font_size,
        &[run],
        Some(renderer.cell_width),
    );
    shaped.push(CachedTextBatch {
        start_column: batch.start_column,
        line,
    });
}

fn should_paint_native_glyph(cell: &PaintCell) -> bool {
    cell.zerowidth.is_none()
        && !cell.flags.contains(Flags::HIDDEN)
        && native_glyph::is_supported(cell.character)
}

fn paint_native_glyph(
    cell: &PaintCell,
    row: usize,
    renderer: &TerminalRenderer,
    origin: Point<Pixels>,
    window: &mut Window,
) {
    let cell_width: f32 = renderer.cell_width.into();
    let cell_height: f32 = renderer.cell_height.into();
    let cell_x = origin.x + renderer.cell_width * cell.column as f32;
    let cell_y = origin.y + renderer.cell_height * row as f32;
    native_glyph::paint_rects(
        cell.character,
        [cell_x.into(), cell_y.into()],
        [cell_width, cell_height],
        window.scale_factor(),
        |x, y, width, height, opacity| {
            window.paint_quad(quad(
                Bounds {
                    origin: Point {
                        x: px(x),
                        y: px(y),
                    },
                    size: Size {
                        width: px(width),
                        height: px(height),
                    },
                },
                px(0.),
                cell.foreground.opacity(opacity),
                Edges::default(),
                transparent_black(),
                Default::default(),
            ));
        },
    );
}

fn paint_cursor(
    cursor: &PaintCursor,
    renderer: &TerminalRenderer,
    origin: Point<Pixels>,
    window: &mut Window,
) {
    let cell_origin = Point {
        x: origin.x + renderer.cell_width * cursor.column as f32,
        y: origin.y + renderer.cell_height * cursor.row as f32,
    };
    let cell_bounds = Bounds {
        origin: cell_origin,
        size: Size {
            width: renderer.cell_width,
            height: renderer.cell_height,
        },
    };
    let cell_width: f32 = renderer.cell_width.into();
    let cell_height: f32 = renderer.cell_height.into();
    let vertical = px((cell_width * 0.15).round().max(1.));
    let horizontal = px((cell_height * 0.15).round().max(1.));

    match cursor.shape {
        CursorShape::Block => paint_cursor_quad(cell_bounds, cursor.color, window),
        CursorShape::Beam => paint_cursor_quad(
            Bounds {
                origin: cell_origin,
                size: Size {
                    width: vertical,
                    height: renderer.cell_height,
                },
            },
            cursor.color,
            window,
        ),
        CursorShape::Underline => paint_cursor_quad(
            Bounds {
                origin: Point {
                    x: cell_origin.x,
                    y: cell_origin.y + renderer.cell_height - horizontal,
                },
                size: Size {
                    width: renderer.cell_width,
                    height: horizontal,
                },
            },
            cursor.color,
            window,
        ),
        CursorShape::HollowBlock => {
            paint_cursor_quad(
                Bounds {
                    origin: cell_origin,
                    size: Size {
                        width: renderer.cell_width,
                        height: horizontal,
                    },
                },
                cursor.color,
                window,
            );
            paint_cursor_quad(
                Bounds {
                    origin: Point {
                        x: cell_origin.x,
                        y: cell_origin.y + renderer.cell_height - horizontal,
                    },
                    size: Size {
                        width: renderer.cell_width,
                        height: horizontal,
                    },
                },
                cursor.color,
                window,
            );
            paint_cursor_quad(
                Bounds {
                    origin: cell_origin,
                    size: Size {
                        width: vertical,
                        height: renderer.cell_height,
                    },
                },
                cursor.color,
                window,
            );
            paint_cursor_quad(
                Bounds {
                    origin: Point {
                        x: cell_origin.x + renderer.cell_width - vertical,
                        y: cell_origin.y,
                    },
                    size: Size {
                        width: vertical,
                        height: renderer.cell_height,
                    },
                },
                cursor.color,
                window,
            );
        }
        CursorShape::Hidden => {}
    }
}

fn paint_cursor_quad(bounds: Bounds<Pixels>, color: Hsla, window: &mut Window) {
    window.paint_quad(quad(
        bounds,
        px(0.),
        color,
        Edges::default(),
        transparent_black(),
        Default::default(),
    ));
}
