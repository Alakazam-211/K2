use parking_lot::Mutex;
use std::borrow::Cow;
use std::collections::HashMap;
use std::os::unix::io::AsRawFd;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use alacritty_terminal::event::{Event as AlacEvent, EventListener, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, EventLoopSender, Msg};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags as CellFlags;
use alacritty_terminal::term::Config as TermConfig;
use alacritty_terminal::tty;
use alacritty_terminal::Term;
use alacritty_terminal::vte::ansi::{Color as AnsiColor, NamedColor, Rgb};

use super::event_sink::TerminalEventSink;
use super::font_renderer::GlyphCache;
use super::grid_types::*;

// ── K2SO Event Listener ──────────────────────────────────────────────────

/// Event listener that forwards alacritty terminal events to the active
/// [`TerminalEventSink`] (or onto the wakeup channel, for grid-redraw
/// triggers).
#[derive(Clone)]
struct K2SOListener {
    wakeup_tx: mpsc::Sender<()>,
    event_sink: Arc<dyn TerminalEventSink>,
    id: String,
}

impl EventListener for K2SOListener {
    fn send_event(&self, event: AlacEvent) {
        match event {
            AlacEvent::Wakeup => {
                let _ = self.wakeup_tx.send(());
            }
            AlacEvent::Title(title) => {
                self.event_sink.on_title(&self.id, &title);
            }
            AlacEvent::Bell => {
                self.event_sink.on_bell(&self.id);
            }
            AlacEvent::ChildExit(status) => {
                let code = status.code().unwrap_or(-1);
                self.event_sink.on_exit(&self.id, code);
            }
            AlacEvent::Exit => {
                self.event_sink.on_exit(&self.id, 0);
            }
            _ => {}
        }
    }
}

// ── Terminal Size Helper ─────────────────────────────────────────────────

struct TermSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows + 5000 // scrollback
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

// ── Shell escaping ──────────────────────────────────────────────────────

/// Shell-escape a single argument for use in a `-c` shell string.
/// Wraps in single quotes and escapes any embedded single quotes.
fn shell_escape_arg(arg: &str) -> String {
    if arg.is_empty() {
        return "''".to_string();
    }
    // If the arg contains no special chars, return as-is
    if arg.chars().all(|c| c.is_alphanumeric() || "-_./=:@,%+".contains(c)) {
        return arg.to_string();
    }
    // Wrap in single quotes, escaping any embedded single quotes: ' → '\''
    format!("'{}'", arg.replace('\'', "'\\''"))
}

// ── Default terminal colors (VS Code Dark+) ─────────────────────────────

fn default_color_palette() -> [Option<Rgb>; 269] {
    let mut colors = [None; 269];

    // Named colors (0-15)
    // Standard ANSI colors — matches src/shared/terminal-theme.ts
    colors[NamedColor::Black as usize] = Some(Rgb { r: 0x1e, g: 0x1e, b: 0x1e });
    colors[NamedColor::Red as usize] = Some(Rgb { r: 0xf4, g: 0x47, b: 0x47 });
    colors[NamedColor::Green as usize] = Some(Rgb { r: 0x6a, g: 0x99, b: 0x55 });
    colors[NamedColor::Yellow as usize] = Some(Rgb { r: 0xd7, g: 0xba, b: 0x7d });
    colors[NamedColor::Blue as usize] = Some(Rgb { r: 0x56, g: 0x9c, b: 0xd6 });
    colors[NamedColor::Magenta as usize] = Some(Rgb { r: 0xc5, g: 0x86, b: 0xc0 });
    colors[NamedColor::Cyan as usize] = Some(Rgb { r: 0x9c, g: 0xdc, b: 0xfe });
    colors[NamedColor::White as usize] = Some(Rgb { r: 0xd4, g: 0xd4, b: 0xd4 });
    colors[NamedColor::BrightBlack as usize] = Some(Rgb { r: 0x5a, g: 0x5a, b: 0x5a });
    colors[NamedColor::BrightRed as usize] = Some(Rgb { r: 0xf4, g: 0x47, b: 0x47 });
    colors[NamedColor::BrightGreen as usize] = Some(Rgb { r: 0x6a, g: 0x99, b: 0x55 });
    colors[NamedColor::BrightYellow as usize] = Some(Rgb { r: 0xd7, g: 0xba, b: 0x7d });
    colors[NamedColor::BrightBlue as usize] = Some(Rgb { r: 0x56, g: 0x9c, b: 0xd6 });
    colors[NamedColor::BrightMagenta as usize] = Some(Rgb { r: 0xc5, g: 0x86, b: 0xc0 });
    colors[NamedColor::BrightCyan as usize] = Some(Rgb { r: 0x9c, g: 0xdc, b: 0xfe });
    colors[NamedColor::BrightWhite as usize] = Some(Rgb { r: 0xff, g: 0xff, b: 0xff });

    // Foreground/Background — matches TERMINAL_COLORS in terminal-theme.ts
    colors[NamedColor::Foreground as usize] = Some(Rgb { r: 0xe0, g: 0xe0, b: 0xe0 });
    colors[NamedColor::Background as usize] = Some(Rgb { r: 0x0a, g: 0x0a, b: 0x0a });
    colors[NamedColor::Cursor as usize] = Some(Rgb { r: 0x52, g: 0x8b, b: 0xff });

    // Standard 256-color palette (indices 16-255)
    // Colors 16-231: 6x6x6 color cube
    for r in 0..6u8 {
        for g in 0..6u8 {
            for b in 0..6u8 {
                let idx = 16 + (r as usize * 36) + (g as usize * 6) + b as usize;
                let rv = if r == 0 { 0 } else { 55 + r * 40 };
                let gv = if g == 0 { 0 } else { 55 + g * 40 };
                let bv = if b == 0 { 0 } else { 55 + b * 40 };
                if idx < 269 {
                    colors[idx] = Some(Rgb { r: rv, g: gv, b: bv });
                }
            }
        }
    }
    // Colors 232-255: grayscale ramp
    for i in 0..24u8 {
        let v = 8 + i * 10;
        let idx = 232 + i as usize;
        if idx < 269 {
            colors[idx] = Some(Rgb { r: v, g: v, b: v });
        }
    }

    colors
}

/// Resolve an alacritty Color to an RGB value using the terminal color palette.
fn resolve_color(color: &AnsiColor, palette: &[Option<Rgb>; 269]) -> u32 {
    match color {
        AnsiColor::Spec(rgb) => ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32,
        AnsiColor::Named(name) => {
            let idx = *name as usize;
            if let Some(rgb) = palette.get(idx).and_then(|c| *c) {
                ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32
            } else {
                0xcccccc // fallback
            }
        }
        AnsiColor::Indexed(idx) => {
            if let Some(rgb) = palette.get(*idx as usize).and_then(|c| *c) {
                ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32
            } else {
                0xcccccc // fallback
            }
        }
    }
}

/// Convert cell flags to our compact u8 representation.
fn flags_to_u8(flags: CellFlags) -> u8 {
    let mut out: u8 = 0;
    if flags.contains(CellFlags::BOLD) {
        out |= ATTR_BOLD;
    }
    if flags.contains(CellFlags::ITALIC) {
        out |= ATTR_ITALIC;
    }
    if flags.intersects(CellFlags::UNDERLINE | CellFlags::DOUBLE_UNDERLINE | CellFlags::UNDERCURL) {
        out |= ATTR_UNDERLINE;
    }
    if flags.contains(CellFlags::STRIKEOUT) {
        out |= ATTR_STRIKETHROUGH;
    }
    if flags.contains(CellFlags::INVERSE) {
        out |= ATTR_INVERSE;
    }
    if flags.contains(CellFlags::DIM) {
        out |= ATTR_DIM;
    }
    if flags.contains(CellFlags::HIDDEN) {
        out |= ATTR_HIDDEN;
    }
    if flags.contains(CellFlags::WIDE_CHAR) {
        out |= ATTR_WIDE;
    }
    out
}

// ── Glyph State ─────────────────────────────────────────────────────────

/// Per-terminal glyph cache wrapper. Tracks font-size + DPR scaling and
/// derived cell metrics used by the grid emission loop for layout. Was
/// previously `GlyphState` when K2SO also shipped an experimental GPU
/// bitmap renderer — that code was removed in 0.32.11; this struct only
/// retains the cell-metrics glyph cache that the DOM grid path still uses.
pub struct GlyphState {
    pub glyph_cache: GlyphCache,
}

// ── Terminal Instance ────────────────────────────────────────────────────

struct AlacrittyTerminalInstance {
    term: Arc<FairMutex<Term<K2SOListener>>>,
    event_loop_sender: EventLoopSender,
    /// Stored for future use — emission threads receive their own clone.
    /// Keeping the handle on the Instance means lifecycle paths that
    /// need to emit lifecycle events ("terminal about to die", etc.)
    /// don't need to thread a separate handle through every method.
    #[allow(dead_code)]
    /// Sink the listener + emission loop forward events through. Held on
    /// the instance so lifecycle paths can reuse it if they ever grow
    /// their own emissions.
    #[allow(dead_code)]
    event_sink: Arc<dyn TerminalEventSink>,
    cwd: String,
    pty_raw_fd: i32,
    child_pid: Option<u32>,
    palette: [Option<Rgb>; 269],
    glyph_state: Arc<Mutex<GlyphState>>,
    /// Atomic flag for grid emission loop — set by scroll() to force full re-snapshot.
    force_full_render: Arc<std::sync::atomic::AtomicBool>,
    /// Channel to send manual wakeups to the emission loop
    /// (e.g. after scroll_display which doesn't trigger PTY wakeups).
    wakeup_tx: Option<mpsc::Sender<()>>,
    /// Handle for the grid emission thread, joined on kill.
    grid_thread_handle: Option<thread::JoinHandle<()>>,
    /// Monotonic counter bumped every time the grid emission loop fires an
    /// update. Consumers (the companion poll loop, the frontend) compare
    /// this value against their last-seen value to decide whether to
    /// re-broadcast. Starts at 1 so `0` remains a sentinel meaning "never
    /// observed". Incremented with Relaxed ordering — we only need
    /// monotonic-per-writer and visibility; no happens-before constraint
    /// with other state.
    seqno: Arc<std::sync::atomic::AtomicU64>,
}

// ── Terminal Manager ─────────────────────────────────────────────────────

pub struct TerminalManager {
    terminals: HashMap<String, AlacrittyTerminalInstance>,
}

impl TerminalManager {
    pub fn new() -> Self {
        Self {
            terminals: HashMap::new(),
        }
    }

    pub fn create(
        &mut self,
        id: String,
        cwd: String,
        command: Option<String>,
        args: Option<Vec<String>>,
        cols: Option<u16>,
        rows: Option<u16>,
        event_sink: Arc<dyn TerminalEventSink>,
    ) -> Result<(), String> {
        if self.terminals.contains_key(&id) {
            log_debug!("[terminal/alacritty] Terminal {} already exists, skipping creation", id);
            return Ok(());
        }

        let safe_cwd = super::resolve_cwd(&cwd);
        let shell = super::detect_shell();

        let c = cols.unwrap_or(80) as usize;
        let r = rows.unwrap_or(24) as usize;

        // Build terminal config
        let term_config = TermConfig {
            scrolling_history: 5000,
            ..TermConfig::default()
        };

        // Create event listener
        let (wakeup_tx, wakeup_rx) = mpsc::channel();
        // Clone wakeup_tx before it's moved into the listener.
        // Scroll uses this to inject wakeups into the same channel as PTY events.
        let scroll_wakeup_tx = wakeup_tx.clone();
        let listener = K2SOListener {
            wakeup_tx,
            event_sink: event_sink.clone(),
            id: id.clone(),
        };

        let term_size = TermSize { cols: c, rows: r };

        // Create Term
        let term = Term::new(term_config, &term_size, listener.clone());
        let term = Arc::new(FairMutex::new(term));

        // Build PTY options
        let mut pty_options = tty::Options {
            working_directory: Some(std::path::PathBuf::from(&safe_cwd)),
            drain_on_exit: true,
            ..Default::default()
        };

        // Set shell and optional command
        if let Some(ref user_command) = command {
            let mut shell_cmd = shell_escape_arg(user_command);
            if let Some(ref user_args) = args {
                for arg in user_args {
                    shell_cmd.push(' ');
                    shell_cmd.push_str(&shell_escape_arg(arg));
                }
            }
            let full_args = vec!["-ilc".to_string(), shell_cmd];
            pty_options.shell = Some(tty::Shell::new(shell, full_args));
        } else {
            pty_options.shell = Some(tty::Shell::new(shell, vec![]));
        }

        // Environment variables
        pty_options.env.insert("TERM".to_string(), "xterm-256color".to_string());
        pty_options.env.insert("TERM_PROGRAM".to_string(), "K2SO".to_string());
        pty_options.env.insert("COLORTERM".to_string(), "truecolor".to_string());
        pty_options.env.insert("PROMPT_EOL_MARK".to_string(), String::new());

        // Agent lifecycle hook env vars
        let hook_port = crate::hook_config::get_port();
        if hook_port > 0 {
            // 0.40.0 rebrand: emit BOTH K2_* (canonical) and K2SO_* (legacy,
            // removed before 1.0.0) so user hook scripts keep working.
            pty_options.env.insert("K2SO_PORT".to_string(), hook_port.to_string());
            pty_options.env.insert("K2_PORT".to_string(), hook_port.to_string());
            pty_options.env.insert("K2SO_PANE_ID".to_string(), id.clone());
            pty_options.env.insert("K2_PANE_ID".to_string(), id.clone());
            pty_options.env.insert("K2SO_TAB_ID".to_string(), id.clone());
            pty_options.env.insert("K2_TAB_ID".to_string(), id.clone());
            pty_options.env.insert("K2SO_HOOK_TOKEN".to_string(), crate::hook_config::get_token().to_string());
            pty_options.env.insert("K2_HOOK_TOKEN".to_string(), crate::hook_config::get_token().to_string());
        }

        // K2SO CLI: add cli/ directory to PATH so agents can call `k2so` commands
        // Search order:
        //   1. Bundled resources: K2SO.app/Contents/Resources/_up_/cli/ (production)
        //   2. Repo root: ../../cli/ relative to binary (development)
        if let Ok(exe_path) = std::env::current_exe() {
            let cli_dir = if let Some(macos_dir) = exe_path.parent() {
                // Production: K2SO.app/Contents/MacOS/k2so → Contents/Resources/_up_/cli/
                // Tauri puts "../cli/*" resources under Resources/_up_/cli/
                let resources_cli = macos_dir.parent()
                    .map(|contents| contents.join("Resources").join("_up_").join("cli"));
                if resources_cli.as_ref().map_or(false, |p| p.exists()) {
                    resources_cli
                } else {
                    // Development: target/debug/k2so → ../../cli/
                    macos_dir.parent().and_then(|p| p.parent())
                        .map(|repo| repo.join("cli"))
                        .filter(|p| p.exists())
                }
            } else {
                None
            };

            if let Some(cli_dir) = cli_dir {
                let existing_path = std::env::var("PATH").unwrap_or_default();
                pty_options.env.insert(
                    "PATH".to_string(),
                    format!("{}:{}", cli_dir.to_string_lossy(), existing_path),
                );
            }
        }

        // Set project path so the CLI knows which workspace it's operating in.
        // Resolve the project root by walking up from cwd to find .k2so/ or .git/,
        // similar to how `git` finds its repo root. This ensures k2so CLI commands
        // work correctly even when the terminal cwd is a worktree or subdirectory.
        let project_root = {
            let mut dir = std::path::PathBuf::from(&safe_cwd);
            let mut found = None;
            for _ in 0..20 { // safety limit
                if crate::workspace_dot_dir(&dir).is_dir() {
                    found = Some(dir.to_string_lossy().to_string());
                    break;
                }
                if !dir.pop() { break; }
            }
            found.unwrap_or_else(|| safe_cwd.clone())
        };
        pty_options.env.insert("K2SO_PROJECT_PATH".to_string(), project_root.clone());
        pty_options.env.insert("K2_PROJECT_PATH".to_string(), project_root);

        // Strip unwanted env vars
        for (key, _) in std::env::vars() {
            if key.starts_with("ELECTRON_") || key.starts_with("VITE_") || key.starts_with("__vite") {
                // alacritty_terminal env is additive, we can't remove from inherited env here
                // Set them to empty as a workaround
                pty_options.env.insert(key, String::new());
            }
        }

        let window_size = WindowSize {
            num_lines: r as u16,
            num_cols: c as u16,
            cell_width: 8,   // approximate, frontend will send real metrics
            cell_height: 16, // approximate
        };

        // Create PTY via alacritty's tty module
        let pty = tty::new(&pty_options, window_size, 0)
            .map_err(|e| format!("Failed to create PTY: {}", e))?;

        let raw_fd = pty.file().as_raw_fd();
        let child_pid = pty.child().id();

        // Create event loop
        let event_loop = EventLoop::new(
            Arc::clone(&term),
            listener,
            pty,
            true,  // drain_on_exit
            false, // ref_test
        )
        .map_err(|e| format!("Failed to create event loop: {}", e))?;

        let event_loop_sender = event_loop.channel();

        // Spawn the event loop thread (reads PTY, parses VT100, updates Term)
        event_loop.spawn();

        let palette = default_color_palette();

        // Initialize glyph cache (used for cell-metrics lookup by the
        // DOM grid emission loop + set_font_size command).
        let font_size = 13.0f32; // matches TERMINAL_FONT_SIZE_DEFAULT
        let dpr = 1.0f32; // updated by frontend on first connect
        let glyph_cache = GlyphCache::new(font_size, dpr);
        let glyph_state = Arc::new(Mutex::new(GlyphState { glyph_cache }));

        let force_full_render = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seqno = Arc::new(std::sync::atomic::AtomicU64::new(1));

        // Spawn grid (DOM text) emission thread
        let term_for_grid = Arc::clone(&term);
        let sink_for_grid = event_sink.clone();
        let id_for_grid = id.clone();
        let grid_palette = palette;
        let force_full_for_grid = Arc::clone(&force_full_render);
        let seqno_for_grid = Arc::clone(&seqno);

        let grid_thread_handle = thread::spawn(move || {
            grid_emission_loop(
                &id_for_grid,
                &term_for_grid,
                &sink_for_grid,
                wakeup_rx,
                grid_palette,
                force_full_for_grid,
                seqno_for_grid,
            );
        });

        let instance = AlacrittyTerminalInstance {
            term,
            event_loop_sender,
            event_sink,
            cwd: safe_cwd,
            pty_raw_fd: raw_fd,
            child_pid: Some(child_pid),
            palette,
            glyph_state,
            force_full_render,
            wakeup_tx: Some(scroll_wakeup_tx),
            grid_thread_handle: Some(grid_thread_handle),
            seqno,
        };

        self.terminals.insert(id.clone(), instance);
        log_debug!("[terminal/alacritty] Terminal {} created ({}x{}, pid={})", id, c, r, child_pid);

        Ok(())
    }

    pub fn write(&self, id: &str, data: &str) -> Result<(), String> {
        let instance = self
            .terminals
            .get(id)
            .ok_or_else(|| format!("Terminal {} not found", id))?;

        // Snap to bottom on user input — like iTerm2 / Zed.
        // If the user scrolled up into history, typing should bring
        // the viewport back to the live terminal.
        {
            let mut term = instance.term.lock_unfair();
            let display_offset = term.grid().display_offset();
            if display_offset != 0 {
                term.scroll_display(alacritty_terminal::grid::Scroll::Bottom);
            }
        }

        let bytes = data.as_bytes().to_vec();
        let _ = instance
            .event_loop_sender
            .send(Msg::Input(Cow::Owned(bytes)));

        Ok(())
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let instance = match self.terminals.get(id) {
            Some(i) => i,
            None => return Ok(()),
        };

        let window_size = WindowSize {
            num_lines: rows,
            num_cols: cols,
            cell_width: 8,
            cell_height: 16,
        };

        // 1. Resize the PTY fd (sends ioctl TIOCSWINSZ, kernel delivers SIGWINCH)
        let _ = instance
            .event_loop_sender
            .send(Msg::Resize(window_size));

        // 2. Resize the terminal grid (reflows content, adjusts cursor position).
        //    Both Alacritty and Zed do this from outside the event loop — the Term
        //    is behind a FairMutex so there's no race condition.
        struct TermSize(usize, usize);
        impl Dimensions for TermSize {
            fn total_lines(&self) -> usize { self.0 }
            fn screen_lines(&self) -> usize { self.0 }
            fn columns(&self) -> usize { self.1 }
        }
        let mut term = instance.term.lock();
        term.resize(TermSize(rows as usize, cols as usize));

        Ok(())
    }

    pub fn kill(&mut self, id: &str) -> Result<(), String> {
        if let Some(mut instance) = self.terminals.remove(id) {
            // Send shutdown to event loop
            let _ = instance.event_loop_sender.send(Msg::Shutdown);

            // Drop the wakeup channel to unblock the grid emission thread
            instance.wakeup_tx.take();

            // Kill child process (Zed pattern: two-phase kill with proper reaping)
            if let Some(pid) = instance.child_pid {
                #[cfg(unix)]
                unsafe {
                    // Phase 1: SIGHUP to process group (graceful)
                    let pgid = libc::getpgid(pid as i32);
                    if pgid > 0 {
                        if libc::killpg(pgid, libc::SIGHUP) != 0 {
                            // Process group kill failed, try direct kill
                            libc::kill(pid as i32, libc::SIGHUP);
                        }
                    } else {
                        libc::kill(pid as i32, libc::SIGHUP);
                    }
                }
                thread::sleep(Duration::from_millis(100));
                #[cfg(unix)]
                unsafe {
                    // Phase 2: SIGKILL (forceful)
                    libc::kill(pid as i32, libc::SIGKILL);
                }

                // Reap the child process to prevent zombies.
                // Retry waitpid a few times since SIGKILL is async — the process may not
                // have exited yet after the first call (Zed pattern: timeout-based reaping).
                #[cfg(unix)]
                unsafe {
                    let mut status: i32 = 0;
                    let mut reaped = false;
                    for _ in 0..5 {
                        let result = libc::waitpid(pid as i32, &mut status, libc::WNOHANG);
                        if result > 0 || result == -1 {
                            reaped = true;
                            break; // Reaped or error (already reaped by another thread)
                        }
                        thread::sleep(Duration::from_millis(20));
                    }
                    if !reaped {
                        // Last resort: blocking waitpid with short timeout by trying once more
                        libc::waitpid(pid as i32, &mut status, libc::WNOHANG);
                    }
                }
            }

            // Join the grid thread with a timeout to prevent thread leaks.
            // Zed pattern: graceful shutdown then timeout-based force cleanup.
            if let Some(grid_handle) = instance.grid_thread_handle.take() {
                // Spawn a joiner thread with 500ms timeout
                let (join_tx, join_rx) = std::sync::mpsc::channel();
                let joiner = thread::spawn(move || {
                    let _ = grid_handle.join();
                    let _ = join_tx.send(());
                });
                if join_rx.recv_timeout(Duration::from_millis(500)).is_err() {
                    // Grid thread didn't exit in time — detach and move on
                    // (thread will exit eventually when its Arc<Term> is the last ref)
                    drop(joiner);
                }
            }
            // instance is dropped here.
        }
        Ok(())
    }

    #[cfg(unix)]
    pub fn kill_foreground(&self, id: &str) -> Result<(), String> {
        let instance = self
            .terminals
            .get(id)
            .ok_or_else(|| format!("Terminal {} not found", id))?;

        let fg_pgid = unsafe { libc::tcgetpgrp(instance.pty_raw_fd) };

        if fg_pgid > 0 {
            unsafe {
                libc::killpg(fg_pgid, libc::SIGINT);
            }
            Ok(())
        } else {
            Err("Could not determine foreground process group".to_string())
        }
    }

    #[cfg(unix)]
    pub fn get_foreground_command(&self, id: &str) -> Result<Option<String>, String> {
        let instance = match self.terminals.get(id) {
            Some(i) => i,
            None => return Ok(None),
        };

        let fg_pgid = unsafe { libc::tcgetpgrp(instance.pty_raw_fd) };
        if fg_pgid <= 0 {
            return Ok(None);
        }

        // If foreground is the shell, return None
        if let Some(pid) = instance.child_pid {
            if fg_pgid == pid as i32 {
                return Ok(None);
            }
        }

        #[cfg(target_os = "macos")]
        {
            let mut buf = [0u8; 4096];
            let ret = unsafe {
                libc::proc_pidpath(
                    fg_pgid,
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len() as u32,
                )
            };
            if ret > 0 {
                let path = String::from_utf8_lossy(&buf[..ret as usize]);
                let name = path.rsplit('/').next().unwrap_or("");
                if !name.is_empty() {
                    return Ok(Some(name.to_string()));
                }
            }
        }

        #[cfg(target_os = "linux")]
        {
            if let Ok(comm) = std::fs::read_to_string(format!("/proc/{}/comm", fg_pgid)) {
                let name = comm.trim();
                if !name.is_empty() {
                    return Ok(Some(name.to_string()));
                }
            }
        }

        Ok(None)
    }

    pub fn exists(&self, id: &str) -> bool {
        self.terminals.contains_key(id)
    }

    /// Read the last N lines of text from the terminal buffer (visible screen).
    /// Uses the same grid access pattern as snapshot_grid for correctness.
    pub fn read_lines(&self, id: &str, count: usize) -> Result<Vec<String>, String> {
        self.read_lines_with_scrollback(id, count, false)
    }

    /// Read terminal lines, optionally including scrollback history.
    /// When `scrollback` is true, reads from the scrollback buffer (up to `count` lines).
    /// When false, reads only the visible screen (current behavior).
    pub fn read_lines_with_scrollback(&self, id: &str, count: usize, scrollback: bool) -> Result<Vec<String>, String> {
        let instance = self.terminals.get(id)
            .ok_or_else(|| format!("Terminal {} not found", id))?;
        let term = instance.term.lock_unfair();
        let content = term.renderable_content();
        let grid = term.grid();
        let screen_lines = grid.screen_lines();
        let cols = grid.columns();
        let display_offset = content.display_offset;

        if scrollback {
            // Read from scrollback history + visible screen
            use alacritty_terminal::grid::Dimensions;
            let history = grid.history_size();
            let total = history + screen_lines;
            let read_count = count.min(total);

            let mut all_lines = Vec::with_capacity(read_count);
            // Start from the scrollback, going from oldest to newest
            let start_line = if total > read_count { total - read_count } else { 0 };
            for i in start_line..total {
                // Negative indices = scrollback, positive = visible screen
                let line_idx = i as i32 - history as i32;
                let line = alacritty_terminal::index::Line(line_idx);
                let row = &grid[line];
                let mut text = String::with_capacity(cols);
                for col in 0..cols {
                    let cell = &row[alacritty_terminal::index::Column(col)];
                    text.push(cell.c);
                }
                all_lines.push(text.trim_end().to_string());
            }

            // Trim trailing empty lines
            while all_lines.last().map_or(false, |l| l.is_empty()) {
                all_lines.pop();
            }
            Ok(all_lines)
        } else {
            // Read only visible screen lines (original behavior)
            let mut all_lines = Vec::with_capacity(screen_lines);
            for row_idx in 0..screen_lines {
                let line = alacritty_terminal::index::Line(row_idx as i32 - display_offset as i32);
                let row = &grid[line];
                let mut text = String::with_capacity(cols);
                for col in 0..cols {
                    let cell = &row[alacritty_terminal::index::Column(col)];
                    text.push(cell.c);
                }
                all_lines.push(text.trim_end().to_string());
            }

            let last_non_empty = all_lines.iter().rposition(|l| !l.is_empty()).unwrap_or(0);
            let start = if last_non_empty + 1 > count { last_non_empty + 1 - count } else { 0 };
            let result: Vec<String> = all_lines[start..=last_non_empty].to_vec();
            Ok(result)
        }
    }

    /// List all terminal IDs and their CWD.
    pub fn list_terminal_ids(&self) -> Vec<(String, String)> {
        self.terminals.iter().map(|(id, inst)| (id.clone(), inst.cwd.clone())).collect()
    }

    /// Get a full grid snapshot for the terminal.
    ///
    /// The returned `GridUpdate.seqno` reflects the latest value bumped by
    /// the emission loop. Consumers (the companion poll loop) compare this
    /// against their last-seen seqno to skip all downstream work when the
    /// grid hasn't changed since the last broadcast.
    pub fn get_grid(&self, id: &str) -> Result<GridUpdate, String> {
        let instance = self
            .terminals
            .get(id)
            .ok_or_else(|| format!("Terminal {} not found", id))?;

        let term = instance.term.lock_unfair();
        let mut update = snapshot_grid(&term, &instance.palette, true);
        update.seqno = instance.seqno.load(std::sync::atomic::Ordering::Relaxed);
        Ok(update)
    }

    /// Scroll the terminal display.
    /// Renders a full bitmap directly and emits it — scroll_display doesn't
    /// reliably produce damage that the emission loop can detect.
    /// Scroll the terminal and trigger a re-render via the emission loop.
    /// We set a flag so the emission loop does a FULL render (not damage-based),
    /// since scroll_display doesn't reliably produce damage.
    pub fn scroll(&self, id: &str, delta: i32) -> Result<(), String> {
        let instance = self
            .terminals
            .get(id)
            .ok_or_else(|| format!("Terminal {} not found", id))?;

        {
            let mut term = instance.term.lock_unfair();
            term.scroll_display(alacritty_terminal::grid::Scroll::Delta(delta));
        }

        // Set the force-full-render flag so emission loop does full snapshot
        instance.force_full_render.store(true, std::sync::atomic::Ordering::Relaxed);

        // Send wakeup through the same channel as PTY events
        // This ensures the frame goes through the emission loop → event system,
        // which is the path that reliably triggers WebKit compositing.
        if let Some(ref wakeup_tx) = instance.wakeup_tx {
            let _ = wakeup_tx.send(());
        }

        Ok(())
    }

    /// Set font size and DPR — invalidates glyph cache, returns new logical cell metrics.
    pub fn set_font_size(&self, id: &str, font_size: f32, dpr: f32) -> Result<(u32, u32), String> {
        let instance = self
            .terminals
            .get(id)
            .ok_or_else(|| format!("Terminal {} not found", id))?;

        let mut bstate = instance.glyph_state.lock();
        bstate.glyph_cache.set_font_size(font_size, dpr);
        Ok((bstate.glyph_cache.logical_cell_width(), bstate.glyph_cache.logical_cell_height()))
    }

    /// Get logical cell metrics for mouse coordinate mapping.
    pub fn get_cell_metrics(&self, id: &str) -> Result<(u32, u32, u16, u16), String> {
        let instance = self
            .terminals
            .get(id)
            .ok_or_else(|| format!("Terminal {} not found", id))?;

        let bstate = instance.glyph_state.lock();
        let term = instance.term.lock_unfair();
        let grid = term.grid();

        Ok((
            bstate.glyph_cache.logical_cell_width(),
            bstate.glyph_cache.logical_cell_height(),
            grid.columns() as u16,
            grid.screen_lines() as u16,
        ))
    }

    /// Set terminal focus state. Historically this controlled cursor
    /// blink state on the experimental bitmap renderer (since removed
    /// in 0.32.11); the DOM grid emission loop relies on the frontend
    /// for focus visuals. Kept as a stable Tauri-command surface so
    /// the frontend's `terminal_set_focus` call continues to resolve
    /// while the bitmap path is reintroduced in a future release.
    pub fn set_focus(&self, id: &str, _focused: bool) -> Result<(), String> {
        if !self.terminals.contains_key(id) {
            return Err(format!("Terminal {} not found", id));
        }
        Ok(())
    }

    /// Get text content from a selection range.
    pub fn get_selection_text(
        &self,
        id: &str,
        start_col: u16,
        start_row: u16,
        end_col: u16,
        end_row: u16,
    ) -> Result<String, String> {
        let instance = self
            .terminals
            .get(id)
            .ok_or_else(|| format!("Terminal {} not found", id))?;

        let term = instance.term.lock_unfair();
        let grid = term.grid();
        let cols = grid.columns();
        let screen_lines = grid.screen_lines();
        let mut text = String::new();

        // Early return for empty grid (prevents out-of-bounds access)
        if cols == 0 || screen_lines == 0 {
            return Ok(String::new());
        }

        let (sr, sc, er, ec) = if start_row < end_row || (start_row == end_row && start_col <= end_col) {
            (start_row, start_col, end_row, end_col)
        } else {
            (end_row, end_col, start_row, start_col)
        };

        for row_idx in sr..=er {
            if row_idx as usize >= screen_lines {
                break;
            }
            let line = Line(row_idx as i32);
            let row = &grid[line];

            let col_start = if row_idx == sr { (sc as usize).min(cols - 1) } else { 0 };
            let col_end = if row_idx == er {
                (ec as usize).min(cols - 1)
            } else {
                cols - 1
            };

            for col in col_start..=col_end {
                let cell = &row[Column(col)];
                if !cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                    text.push(cell.c);
                    if let Some(zws) = cell.zerowidth() {
                        for &zw in zws {
                            text.push(zw);
                        }
                    }
                }
            }

            // Add newline between rows (but not after last row)
            if row_idx < er {
                // Trim trailing spaces from line before adding newline
                let trimmed_len = text.trim_end().len();
                text.truncate(trimmed_len);
                text.push('\n');
            }
        }

        // Trim trailing whitespace from final result
        let trimmed = text.trim_end().to_string();
        Ok(trimmed)
    }

    pub fn get_count_for_path(&self, path: &str) -> i32 {
        self.terminals
            .values()
            .filter(|inst| inst.cwd.starts_with(path))
            .count() as i32
    }

    pub fn kill_all(&mut self) {
        let ids: Vec<String> = self.terminals.keys().cloned().collect();
        for id in ids {
            let _ = self.kill(&id);
        }
    }
}

impl Drop for TerminalManager {
    fn drop(&mut self) {
        self.kill_all();
    }
}

impl Default for TerminalManager {
    fn default() -> Self {
        Self::new()
    }
}

// ── Grid Snapshot ────────────────────────────────────────────────────────

/// Default foreground/background colors (must match terminal-theme.ts TERMINAL_COLORS)
const DEFAULT_FG: u32 = 0xe0e0e0;
const DEFAULT_BG: u32 = 0x0a0a0a;

/// Convert a row of alacritty cells to a compact line (text + sparse style spans).
/// Span indices use "text position" (char index into the text string, not grid column),
/// so they stay aligned even when wide char spacers are skipped.
fn row_to_compact_line(
    row_idx: usize,
    row: &alacritty_terminal::grid::Row<alacritty_terminal::term::cell::Cell>,
    cols: usize,
    palette: &[Option<Rgb>; 269],
) -> CompactLine {
    let mut text = String::with_capacity(cols);
    let mut spans: Vec<StyleSpan> = Vec::new();

    // Current span tracking — positions are TEXT indices (not grid columns)
    let mut span_start: Option<u16> = None;
    let mut span_fg: u32 = DEFAULT_FG;
    let mut span_bg: u32 = DEFAULT_BG;
    let mut span_flags: u8 = 0;
    let mut text_pos: u16 = 0;

    // Track the rightmost styled position to avoid trimming styled trailing spaces
    let mut rightmost_styled_text_pos: Option<u16> = None;

    for col_idx in 0..cols {
        let cell = &row[Column(col_idx)];

        if cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
            continue;
        }

        let fg = resolve_color(&cell.fg, palette);
        let bg = resolve_color(&cell.bg, palette);
        let flags = flags_to_u8(cell.flags);

        let current_text_pos = text_pos;
        text.push(cell.c);
        text_pos += 1;
        if let Some(zws) = cell.zerowidth() {
            for &zw in zws {
                text.push(zw);
                // Don't increment text_pos — zero-width chars don't take a cell
            }
        }

        let is_default = fg == DEFAULT_FG && bg == DEFAULT_BG && flags == 0;

        if !is_default {
            rightmost_styled_text_pos = Some(current_text_pos);

            // Extend or start a span
            if let Some(start) = span_start {
                if fg == span_fg && bg == span_bg && flags == span_flags {
                    // Continue current span
                } else {
                    // Flush previous span, start new one
                    spans.push(StyleSpan {
                        s: start,
                        e: current_text_pos.saturating_sub(1),
                        fg: if span_fg != DEFAULT_FG { Some(span_fg) } else { None },
                        bg: if span_bg != DEFAULT_BG { Some(span_bg) } else { None },
                        fl: if span_flags != 0 { Some(span_flags) } else { None },
                    });
                    span_start = Some(current_text_pos);
                    span_fg = fg;
                    span_bg = bg;
                    span_flags = flags;
                }
            } else {
                span_start = Some(current_text_pos);
                span_fg = fg;
                span_bg = bg;
                span_flags = flags;
            }
        } else if let Some(start) = span_start {
            // Flush span — we hit a default cell
            spans.push(StyleSpan {
                s: start,
                e: current_text_pos.saturating_sub(1),
                fg: if span_fg != DEFAULT_FG { Some(span_fg) } else { None },
                bg: if span_bg != DEFAULT_BG { Some(span_bg) } else { None },
                fl: if span_flags != 0 { Some(span_flags) } else { None },
            });
            span_start = None;
        }
    }

    // Flush final span if any
    if let Some(start) = span_start {
        spans.push(StyleSpan {
            s: start,
            e: text_pos.saturating_sub(1),
            fg: if span_fg != DEFAULT_FG { Some(span_fg) } else { None },
            bg: if span_bg != DEFAULT_BG { Some(span_bg) } else { None },
            fl: if span_flags != 0 { Some(span_flags) } else { None },
        });
    }

    // Trim trailing spaces, but only up to the rightmost styled position.
    // This preserves styled spaces (like cursor backgrounds) that would
    // otherwise be lost.
    let trimmed = if let Some(styled_pos) = rightmost_styled_text_pos {
        let min_chars = styled_pos as usize + 1;
        let t = text.trim_end();
        // Compare CHAR count, not byte length — multi-byte chars like ❯ make
        // byte length larger than char count, which would skip the fix.
        let trimmed_chars = t.chars().count();
        if trimmed_chars < min_chars {
            // Keep at least up to the rightmost styled char
            let chars: Vec<char> = text.chars().collect();
            chars[..min_chars.min(chars.len())].iter().collect()
        } else {
            t.to_string()
        }
    } else {
        text.trim_end().to_string()
    };

    // Debug: log rows with background-colored spans (like CLI cursors)
    let has_bg_spans = spans.iter().any(|s| s.bg.is_some());
    if has_bg_spans {
        log_debug!(
            "[compact] row={} text={:?} (len={}) spans={:?} rightmost_styled={:?}",
            row_idx,
            &trimmed,
            trimmed.len(),
            spans.iter().map(|s| format!("s{}..e{} fg={:?} bg={:?} fl={:?}", s.s, s.e, s.fg, s.bg, s.fl)).collect::<Vec<_>>(),
            rightmost_styled_text_pos,
        );
    }

    // Check if the last non-spacer cell has the WRAPLINE flag (soft-wrap indicator).
    // This means this row continues on the next row (the program didn't send a newline).
    let wrapped = {
        let mut is_wrapped = false;
        for col in (0..cols).rev() {
            let cell = &row[Column(col)];
            if !cell.flags.contains(CellFlags::WIDE_CHAR_SPACER) {
                is_wrapped = cell.flags.contains(CellFlags::WRAPLINE);
                break;
            }
        }
        is_wrapped
    };

    CompactLine {
        row: row_idx as u16,
        text: trimmed,
        spans,
        wrapped,
    }
}

/// Take a full snapshot of the terminal grid using compact line format.
fn snapshot_grid(
    term: &Term<K2SOListener>,
    palette: &[Option<Rgb>; 269],
    full: bool,
) -> GridUpdate {
    let start = std::time::Instant::now();

    let content = term.renderable_content();
    let grid = term.grid();
    let cols = grid.columns();
    let rows = grid.screen_lines();
    let display_offset = content.display_offset;
    let cursor_point = content.cursor.point;

    // Read cursor shape from alacritty (Block, Beam, Underline, Hidden, HollowBlock)
    let (cursor_visible, cursor_shape) = match content.cursor.shape {
        alacritty_terminal::vte::ansi::CursorShape::Block => (true, "block"),
        alacritty_terminal::vte::ansi::CursorShape::Underline => (true, "underline"),
        alacritty_terminal::vte::ansi::CursorShape::Beam => (true, "bar"),
        _ => (true, "block"),
    };
    // Hide cursor when scrolled up or when SHOW_CURSOR is off
    let cursor_visible = cursor_visible
        && display_offset == 0
        && term.mode().contains(alacritty_terminal::term::TermMode::SHOW_CURSOR);

    let mut lines = Vec::with_capacity(rows);

    for row_idx in 0..rows {
        let line = Line(row_idx as i32 - display_offset as i32);
        let row = &grid[line];
        let compact = row_to_compact_line(row_idx, row, cols, palette);

        // Skip entirely empty lines to reduce payload
        if !compact.text.is_empty() || !compact.spans.is_empty() || !full {
            lines.push(compact);
        }
    }

    let elapsed = start.elapsed();
    let text_bytes: u32 = lines.iter().map(|l| l.text.len() as u32).sum();
    let span_count: u16 = lines.iter().map(|l| l.spans.len() as u16).sum();

    GridUpdate {
        cols: cols as u16,
        rows: rows as u16,
        cursor_col: cursor_point.column.0 as u16,
        cursor_row: cursor_point.line.0 as u16,
        cursor_visible,
        cursor_shape: cursor_shape.to_string(),
        lines,
        full,
        mode: term.mode().bits(),
        display_offset,
        selection: None,
        perf: Some(PerfInfo {
            snapshot_us: elapsed.as_micros() as u64,
            line_count: rows as u16,
            text_bytes,
            span_count,
        }),
        // `0` signals "seqno not stamped by this path". Callers that own a
        // TerminalInstance (get_grid, the emission loop) overwrite this
        // with the current atomic value.
        seqno: 0,
    }
}

/// Take an incremental snapshot using damage tracking with compact line format.
fn snapshot_damaged(
    term: &mut Term<K2SOListener>,
    palette: &[Option<Rgb>; 269],
) -> GridUpdate {
    let start = std::time::Instant::now();

    // Get cursor info before damage() borrows term
    let content = term.renderable_content();
    let cursor_point = content.cursor.point;
    let display_offset = content.display_offset;
    let (cursor_shape_str, cursor_shape_visible) = match content.cursor.shape {
        alacritty_terminal::vte::ansi::CursorShape::Block => ("block", true),
        alacritty_terminal::vte::ansi::CursorShape::Underline => ("underline", true),
        alacritty_terminal::vte::ansi::CursorShape::Beam => ("bar", true),
        _ => ("block", true),
    };
    let cursor_visible = cursor_shape_visible
        && display_offset == 0
        && term.mode().contains(alacritty_terminal::term::TermMode::SHOW_CURSOR);
    drop(content);

    let grid = term.grid();
    let cols = grid.columns();
    let rows = grid.screen_lines();

    let damage = term.damage();
    let is_full = matches!(damage, alacritty_terminal::term::TermDamage::Full);

    let damaged_lines: Vec<usize> = match damage {
        alacritty_terminal::term::TermDamage::Full => {
            (0..rows).collect()
        }
        alacritty_terminal::term::TermDamage::Partial(iter) => {
            iter.filter(|d| d.is_damaged())
                .map(|d| d.line)
                .collect()
        }
    };

    let grid = term.grid(); // re-borrow after damage()
    let mut lines = Vec::with_capacity(damaged_lines.len());

    for &row_idx in &damaged_lines {
        if row_idx >= rows {
            continue;
        }
        let line = Line(row_idx as i32 - display_offset as i32);
        let row = &grid[line];
        lines.push(row_to_compact_line(row_idx, row, cols, palette));
    }

    term.reset_damage();

    let elapsed = start.elapsed();
    let text_bytes: u32 = lines.iter().map(|l| l.text.len() as u32).sum();
    let span_count: u16 = lines.iter().map(|l| l.spans.len() as u16).sum();
    let line_count = lines.len() as u16;

    GridUpdate {
        cols: cols as u16,
        rows: rows as u16,
        cursor_col: cursor_point.column.0 as u16,
        cursor_row: cursor_point.line.0 as u16,
        cursor_visible,
        cursor_shape: cursor_shape_str.to_string(),
        lines,
        full: is_full,
        mode: term.mode().bits(),
        display_offset,
        selection: None,
        perf: Some(PerfInfo {
            snapshot_us: elapsed.as_micros() as u64,
            line_count,
            text_bytes,
            span_count,
        }),
        seqno: 0, // stamped by the emission loop after this returns
    }
}

// ── Grid (DOM text) Emission Loop ───────────────────────────────────────

/// Background thread that snapshots the terminal grid as styled text runs
/// and emits GridUpdate events to the frontend for DOM rendering.
/// This replaces bitmap rendering with text-based IPC.
fn grid_emission_loop(
    id: &str,
    term: &Arc<FairMutex<Term<K2SOListener>>>,
    event_sink: &Arc<dyn TerminalEventSink>,
    wakeup_rx: mpsc::Receiver<()>,
    palette: [Option<Rgb>; 269],
    force_full_render: Arc<std::sync::atomic::AtomicBool>,
    seqno: Arc<std::sync::atomic::AtomicU64>,
) {
    let min_frame_interval = Duration::from_millis(16);
    let mut last_emit = std::time::Instant::now() - min_frame_interval;

    loop {
        // Block on wakeup channel — both PTY events and scroll use this
        match wakeup_rx.recv() {
            Ok(()) => {}
            Err(_) => break,
        }

        // Rate limit: ensure at least 16ms between emissions
        let since_last = last_emit.elapsed();
        if since_last < min_frame_interval {
            let wait = min_frame_interval - since_last;
            let deadline = std::time::Instant::now() + wait;
            loop {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() { break; }
                match wakeup_rx.recv_timeout(remaining) {
                    Ok(()) => continue,
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
        }

        // Adaptive batch window: 4ms normally, extended during high-throughput
        let mut wakeup_count = 0u32;
        let batch_deadline = std::time::Instant::now() + Duration::from_millis(4);
        loop {
            let remaining = batch_deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() { break; }
            match wakeup_rx.recv_timeout(remaining) {
                Ok(()) => { wakeup_count += 1; continue; }
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }

        // During high-throughput bursts, wait longer to accumulate changes
        if wakeup_count > 10 {
            let burst_deadline = std::time::Instant::now() + Duration::from_millis(30);
            loop {
                let remaining = burst_deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() { break; }
                match wakeup_rx.recv_timeout(remaining) {
                    Ok(()) => continue,
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
        }

        // Snapshot the grid
        let force_full = force_full_render.swap(false, std::sync::atomic::Ordering::Relaxed);

        let mut update = {
            let mut term = term.lock_unfair();

            if force_full {
                // Consume and discard damage, take full snapshot
                let _ = term.damage();
                term.reset_damage();
                snapshot_grid(&term, &palette, true)
            } else {
                snapshot_damaged(&mut term, &palette)
            }
        };

        // Only emit if there are lines to send
        if !update.lines.is_empty() {
            // Bump + stamp seqno. This is the single source of truth for
            // "grid has been updated" — the companion poll loop compares
            // against its last-seen seqno to gate all subsequent work.
            let next = seqno.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            update.seqno = next;
            event_sink.on_grid_update(id, &update);
        }

        last_emit = std::time::Instant::now();
    }
}
