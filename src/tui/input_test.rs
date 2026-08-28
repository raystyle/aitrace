//! Input-path self-test: paint the terminal diagnostics themselves.
//!
//! When keys do not reach the TUI and shell text bleeds through the frame,
//! the cause lives below the application (console handles, raw mode,
//! alternate screen, size detection). This mode renders all of that onto
//! the screen and mirrors it to `.aitrace/input-test.log`, so one run in
//! the affected terminal yields the full picture.

use std::io::{self, Write};

use anyhow::Result;
use crossterm::{
    event::{self, Event},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend, text::Line, widgets::Paragraph};

/// Bit names for the Windows console input mode (0 when not on Windows).
#[cfg(windows)]
fn input_mode_bits(mode: u32) -> Vec<&'static str> {
    use windows_sys::Win32::System::Console::{
        ENABLE_ECHO_INPUT, ENABLE_EXTENDED_FLAGS, ENABLE_INSERT_MODE, ENABLE_LINE_INPUT,
        ENABLE_MOUSE_INPUT, ENABLE_PROCESSED_INPUT, ENABLE_QUICK_EDIT_MODE,
        ENABLE_VIRTUAL_TERMINAL_INPUT, ENABLE_WINDOW_INPUT,
    };
    let mut bits = Vec::new();
    if mode & ENABLE_ECHO_INPUT != 0 {
        bits.push("ECHO");
    }
    if mode & ENABLE_LINE_INPUT != 0 {
        bits.push("LINE");
    }
    if mode & ENABLE_PROCESSED_INPUT != 0 {
        bits.push("PROCESSED");
    }
    if mode & ENABLE_MOUSE_INPUT != 0 {
        bits.push("MOUSE");
    }
    if mode & ENABLE_WINDOW_INPUT != 0 {
        bits.push("WINDOW");
    }
    if mode & ENABLE_QUICK_EDIT_MODE != 0 {
        bits.push("QUICKEDIT");
    }
    if mode & ENABLE_INSERT_MODE != 0 {
        bits.push("INSERT");
    }
    if mode & ENABLE_EXTENDED_FLAGS != 0 {
        bits.push("EXTENDED");
    }
    if mode & ENABLE_VIRTUAL_TERMINAL_INPUT != 0 {
        bits.push("VT-INPUT");
    }
    bits
}

#[cfg(windows)]
fn console_input_mode() -> (u32, bool) {
    use windows_sys::Win32::System::Console::{GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE};
    unsafe {
        let handle = GetStdHandle(STD_INPUT_HANDLE);
        let mut mode: u32 = 0;
        let ok = GetConsoleMode(handle, &mut mode);
        (mode, ok != 0)
    }
}

#[cfg(not(windows))]
fn console_input_mode() -> (u32, bool) {
    (0, false)
}

/// Clear the console input bits that break interactive TUIs even under
/// raw mode: QuickEdit (a mouse click freezes all keyboard input into
/// text-selection mode; right-click pastes the clipboard), INSERT, and
/// legacy MOUSE input (we use VT mouse). EXTENDED must stay set for the
/// QuickEdit/INSERT bits to take effect.
#[cfg(windows)]
pub fn harden_console_input() {
    use windows_sys::Win32::System::Console::{
        ENABLE_EXTENDED_FLAGS, ENABLE_INSERT_MODE, ENABLE_MOUSE_INPUT, ENABLE_QUICK_EDIT_MODE,
        GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, SetConsoleMode,
    };
    unsafe {
        let handle = GetStdHandle(STD_INPUT_HANDLE);
        let mut mode: u32 = 0;
        if GetConsoleMode(handle, &mut mode) == 0 {
            return;
        }
        let cleared = mode & !(ENABLE_QUICK_EDIT_MODE | ENABLE_INSERT_MODE | ENABLE_MOUSE_INPUT)
            | ENABLE_EXTENDED_FLAGS;
        SetConsoleMode(handle, cleared);
    }
}

#[cfg(not(windows))]
pub fn harden_console_input() {}

#[cfg(not(windows))]
fn input_mode_bits(_mode: u32) -> Vec<&'static str> {
    Vec::new()
}

pub fn run() -> Result<()> {
    let mut log = std::fs::File::create(".aitrace/input-test.log")?;

    let (mode_before, ok_before) = console_input_mode();
    let size_before = crossterm::terminal::size().ok();
    writeln!(
        log,
        "stdin mode before raw: {mode_before:#x} ok={ok_before} bits={:?} size={size_before:?}",
        input_mode_bits(mode_before)
    )?;

    enable_raw_mode()?;
    harden_console_input();
    let (mode_after, ok_after) = console_input_mode();
    writeln!(
        log,
        "stdin mode after raw:  {mode_after:#x} ok={ok_after} bits={:?}",
        input_mode_bits(mode_after)
    )?;

    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut events: Vec<String> = Vec::new();
    let started = std::time::Instant::now();

    loop {
        // Drain every pending event each frame.
        let mut got_any = false;
        let mut should_quit = false;
        while event::poll(std::time::Duration::from_millis(0))? {
            if let Ok(ev) = event::read() {
                got_any = true;
                let line = describe(&ev);
                writeln!(log, "+{:.2}s {line}", started.elapsed().as_secs_f32())?;
                if is_quit(&ev) {
                    should_quit = true;
                }
                events.push(format!("+{:.2}s  {line}", started.elapsed().as_secs_f32()));
                if events.len() > 12 {
                    events.remove(0);
                }
            }
        }
        if should_quit {
            break;
        }

        let mut lines: Vec<Line> = vec![
            Line::from("aitrace input self-test -- press keys, they must appear below"),
            Line::from(format!(
                "stdin mode before raw: {:#x} ok={} {:?}",
                mode_before,
                ok_before,
                input_mode_bits(mode_before)
            )),
            Line::from(format!(
                "stdin mode after raw:  {:#x} ok={} {:?}   (ECHO/LINE must be gone)",
                mode_after,
                ok_after,
                input_mode_bits(mode_after)
            )),
            Line::from(format!(
                "terminal size: now {:?} / before raw {:?}  (frame box must span the whole window)",
                crossterm::terminal::size().ok(),
                size_before
            )),
            Line::from(""),
            Line::from("events received (most recent last):"),
        ];
        if events.is_empty() {
            lines.push(Line::from("  <none yet>"));
        } else {
            lines.extend(events.iter().cloned().map(Line::from));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(
            "press q or Ctrl+C to quit (if this line reacts, input works)",
        ));

        terminal.draw(|f| {
            let block = ratatui::widgets::Block::bordered().title(" input-test ");
            f.render_widget(Paragraph::new(lines).block(block), f.area());
        })?;

        if !got_any {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    println!("input test finished -- details in .aitrace/input-test.log");
    Ok(())
}

fn describe(ev: &Event) -> String {
    match ev {
        Event::Key(k) => format!(
            "Key {:?} code={:?} mods={:?} kind={:?} state={:?}",
            k.code, k.code, k.modifiers, k.kind, k.state
        ),
        Event::Mouse(m) => format!("Mouse {m:?}"),
        Event::Resize(w, h) => format!("Resize {w}x{h}"),
        Event::FocusGained => "FocusGained".to_string(),
        Event::FocusLost => "FocusLost".to_string(),
        Event::Paste(p) => format!("Paste({p})"),
        #[allow(unreachable_patterns)]
        _ => "other".to_string(),
    }
}

fn is_quit(ev: &Event) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};
    match ev {
        Event::Key(k) if k.code == KeyCode::Char('q') => {
            !k.modifiers.contains(KeyModifiers::CONTROL)
        }
        Event::Key(k) if k.code == KeyCode::Char('c') => {
            k.modifiers.contains(KeyModifiers::CONTROL)
        }
        _ => false,
    }
}
