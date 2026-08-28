//! Input-path self-test: paint the terminal diagnostics themselves.
//!
//! When keys do not reach the TUI and shell text bleeds through the frame,
//! the cause lives below the application (console handles, raw mode,
//! alternate screen, size detection). This mode renders all of that onto
//! the screen and mirrors it to `.aitrace/input-test.log`, so one run in
//! the affected terminal yields the full picture.

use std::io::{self, Write};
use std::path::Path;

use anyhow::Result;
use crossterm::{
    cursor::Hide,
    event::{self, Event},
    execute,
    terminal::{
        Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode,
        enable_raw_mode,
    },
};
use ratatui::{Terminal, backend::CrosstermBackend, text::Line, widgets::Paragraph};

// Win32 console mode bits (stable values). Kept as integers so the pure
// harden functions and their tests compile on every target.
const ENABLE_PROCESSED_INPUT: u32 = 0x0001;
const ENABLE_LINE_INPUT: u32 = 0x0002;
const ENABLE_ECHO_INPUT: u32 = 0x0004;
const ENABLE_WINDOW_INPUT: u32 = 0x0008;
const ENABLE_MOUSE_INPUT: u32 = 0x0010;
const ENABLE_INSERT_MODE: u32 = 0x0020;
const ENABLE_QUICK_EDIT_MODE: u32 = 0x0040;
const ENABLE_EXTENDED_FLAGS: u32 = 0x0080;
const ENABLE_VIRTUAL_TERMINAL_INPUT: u32 = 0x0200;

const ENABLE_PROCESSED_OUTPUT: u32 = 0x0001;
const ENABLE_WRAP_AT_EOL_OUTPUT: u32 = 0x0002;
const ENABLE_VIRTUAL_TERMINAL_PROCESSING: u32 = 0x0004;
const DISABLE_NEWLINE_AUTO_RETURN: u32 = 0x0008;

/// PE `IMAGE_OPTIONAL_HEADER.Subsystem` values.
pub const IMAGE_SUBSYSTEM_WINDOWS_GUI: u16 = 2;
pub const IMAGE_SUBSYSTEM_WINDOWS_CUI: u16 = 3;

/// Read the PE subsystem field from an image. `None` if the bytes are not a PE.
pub fn pe_subsystem(bytes: &[u8]) -> Option<u16> {
    if bytes.len() < 0x40 {
        return None;
    }
    if bytes[0] != b'M' || bytes[1] != b'Z' {
        return None;
    }
    let e_lfanew = u32::from_le_bytes(bytes[0x3c..0x40].try_into().ok()?) as usize;
    let pe = e_lfanew;
    let opt = pe.checked_add(24)?;
    let sub_off = opt.checked_add(68)?;
    if bytes.len() < sub_off.checked_add(2)? {
        return None;
    }
    if &bytes[pe..pe + 4] != b"PE\0\0" {
        return None;
    }
    let magic = u16::from_le_bytes(bytes[opt..opt + 2].try_into().ok()?);
    if magic != 0x10b && magic != 0x20b {
        return None;
    }
    Some(u16::from_le_bytes(
        bytes[sub_off..sub_off + 2].try_into().ok()?,
    ))
}

/// Read the PE subsystem of a file on disk.
pub fn pe_subsystem_file(path: &Path) -> Option<u16> {
    pe_subsystem(&std::fs::read(path).ok()?)
}

fn subsystem_label(sub: Option<u16>) -> String {
    match sub {
        Some(IMAGE_SUBSYSTEM_WINDOWS_CUI) => {
            "CUI (3) — parent shell waits; keys should reach this process".to_string()
        }
        Some(IMAGE_SUBSYSTEM_WINDOWS_GUI) => {
            "GUI (2) — pwsh will NOT wait; prompt and keys stay in the shell".to_string()
        }
        Some(other) => format!("{other} (unexpected PE subsystem)"),
        None => "unknown (not a PE or unreadable)".to_string(),
    }
}

/// Drop QuickEdit / INSERT / legacy mouse, keep EXTENDED so those bits stick,
/// and turn on VT input so mouse/focus sequences reach the TUI.
pub fn harden_input_mode(mode: u32) -> u32 {
    (mode & !(ENABLE_QUICK_EDIT_MODE | ENABLE_INSERT_MODE | ENABLE_MOUSE_INPUT))
        | ENABLE_EXTENDED_FLAGS
        | ENABLE_VIRTUAL_TERMINAL_INPUT
}

/// CSI alternate-screen (`?1049h`) is ignored unless VT processing is on.
/// DISABLE_NEWLINE_AUTO_RETURN stops the last cell of a full frame from
/// scrolling the buffer and revealing the shell underneath.
pub fn harden_output_mode(mode: u32) -> u32 {
    mode | ENABLE_PROCESSED_OUTPUT
        | ENABLE_WRAP_AT_EOL_OUTPUT
        | ENABLE_VIRTUAL_TERMINAL_PROCESSING
        | DISABLE_NEWLINE_AUTO_RETURN
}

fn input_mode_bits(mode: u32) -> Vec<&'static str> {
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

fn output_mode_bits(mode: u32) -> Vec<&'static str> {
    let mut bits = Vec::new();
    if mode & ENABLE_PROCESSED_OUTPUT != 0 {
        bits.push("PROCESSED");
    }
    if mode & ENABLE_WRAP_AT_EOL_OUTPUT != 0 {
        bits.push("WRAP");
    }
    if mode & ENABLE_VIRTUAL_TERMINAL_PROCESSING != 0 {
        bits.push("VT-OUT");
    }
    if mode & DISABLE_NEWLINE_AUTO_RETURN != 0 {
        bits.push("NO-NL-RETURN");
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

#[cfg(windows)]
fn console_output_mode() -> (u32, bool) {
    use windows_sys::Win32::System::Console::{GetConsoleMode, GetStdHandle, STD_OUTPUT_HANDLE};
    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut mode: u32 = 0;
        let ok = GetConsoleMode(handle, &mut mode);
        (mode, ok != 0)
    }
}

#[cfg(not(windows))]
fn console_output_mode() -> (u32, bool) {
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
        GetConsoleMode, GetStdHandle, STD_INPUT_HANDLE, SetConsoleMode,
    };
    unsafe {
        let handle = GetStdHandle(STD_INPUT_HANDLE);
        let mut mode: u32 = 0;
        if GetConsoleMode(handle, &mut mode) == 0 {
            return;
        }
        SetConsoleMode(handle, harden_input_mode(mode));
    }
}

#[cfg(not(windows))]
pub fn harden_console_input() {}

/// Enable VT processing on stdout so `EnterAlternateScreen` is not a no-op.
#[cfg(windows)]
pub fn harden_console_output() {
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, STD_OUTPUT_HANDLE, SetConsoleMode,
    };
    unsafe {
        let handle = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut mode: u32 = 0;
        if GetConsoleMode(handle, &mut mode) == 0 {
            return;
        }
        SetConsoleMode(handle, harden_output_mode(mode));
    }
}

#[cfg(not(windows))]
pub fn harden_console_output() {}

#[cfg(windows)]
fn console_process_count() -> Option<u32> {
    use windows_sys::Win32::System::Console::GetConsoleProcessList;
    unsafe {
        let mut buf = [0u32; 16];
        let n = GetConsoleProcessList(buf.as_mut_ptr(), buf.len() as u32);
        if n == 0 { None } else { Some(n) }
    }
}

#[cfg(not(windows))]
fn console_process_count() -> Option<u32> {
    None
}

/// Switch to the alternate screen, hide the cursor, and wipe the visible
/// buffer. Ratatui's first diff starts from a blank *internal* buffer; if
/// the physical screen still has the shell prompt, those cells look
/// unchanged and bleed through. The explicit clear closes that gap when
/// CSI `?1049h` is ignored (Windows without VT-OUT).
pub fn take_over_screen(stdout: &mut impl Write) -> io::Result<()> {
    harden_console_output();
    execute!(stdout, EnterAlternateScreen, Hide, Clear(ClearType::All))?;
    Ok(())
}

pub fn run() -> Result<()> {
    let mut log = std::fs::File::create(".aitrace/input-test.log")?;

    let (in_before, in_ok_before) = console_input_mode();
    let (out_before, out_ok_before) = console_output_mode();
    let size_before = crossterm::terminal::size().ok();
    writeln!(
        log,
        "stdin mode before raw: {in_before:#x} ok={in_ok_before} bits={:?} size={size_before:?}",
        input_mode_bits(in_before)
    )?;
    writeln!(
        log,
        "stdout mode before:    {out_before:#x} ok={out_ok_before} bits={:?}",
        output_mode_bits(out_before)
    )?;

    enable_raw_mode()?;
    harden_console_input();
    harden_console_output();
    let (in_after, in_ok_after) = console_input_mode();
    let (out_after, out_ok_after) = console_output_mode();
    writeln!(
        log,
        "stdin mode after raw:  {in_after:#x} ok={in_ok_after} bits={:?}",
        input_mode_bits(in_after)
    )?;
    writeln!(
        log,
        "stdout mode after:     {out_after:#x} ok={out_ok_after} bits={:?}",
        output_mode_bits(out_after)
    )?;
    let exe_sub = std::env::current_exe()
        .ok()
        .and_then(|p| pe_subsystem_file(&p));
    let attached = console_process_count();
    writeln!(
        log,
        "PE subsystem: {}  attached console processes: {:?}",
        subsystem_label(exe_sub),
        attached
    )?;

    let mut stdout = io::stdout();
    take_over_screen(&mut stdout)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

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
                if matches!(ev, Event::Resize(_, _)) {
                    let _ = terminal.clear();
                }
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
            Line::from("the shell prompt must NOT be visible inside this box"),
            Line::from(format!(
                "stdin before: {:#x} ok={} {:?}",
                in_before,
                in_ok_before,
                input_mode_bits(in_before)
            )),
            Line::from(format!(
                "stdin after:  {:#x} ok={} {:?}   (ECHO/LINE must be gone)",
                in_after,
                in_ok_after,
                input_mode_bits(in_after)
            )),
            Line::from(format!(
                "stdout before:{:#x} ok={} {:?}",
                out_before,
                out_ok_before,
                output_mode_bits(out_before)
            )),
            Line::from(format!(
                "stdout after: {:#x} ok={} {:?}   (VT-OUT must be set or alt-screen CSI is ignored)",
                out_after,
                out_ok_after,
                output_mode_bits(out_after)
            )),
            Line::from(format!(
                "terminal size: now {:?} / before raw {:?}  (frame box must span the whole window)",
                crossterm::terminal::size().ok(),
                size_before
            )),
            Line::from(format!("PE subsystem: {}", subsystem_label(exe_sub))),
            Line::from(format!(
                "console processes attached: {:?}  (pwsh+aitrace is normal; keys must still appear below)",
                attached
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Observed Windows cooked mode from `.aitrace/input-test.log`
    /// (ECHO|LINE|PROCESSED|MOUSE|INSERT|QUICKEDIT|EXTENDED|AUTO_POSITION).
    const COOKED_INPUT: u32 = 0x1f7;

    #[test]
    fn harden_input_drops_quickedit_keeps_extended() {
        let out = harden_input_mode(COOKED_INPUT);
        assert_eq!(out & ENABLE_QUICK_EDIT_MODE, 0);
        assert_eq!(out & ENABLE_INSERT_MODE, 0);
        assert_eq!(out & ENABLE_MOUSE_INPUT, 0);
        assert_eq!(out & ENABLE_EXTENDED_FLAGS, ENABLE_EXTENDED_FLAGS);
        assert_eq!(
            out & ENABLE_VIRTUAL_TERMINAL_INPUT,
            ENABLE_VIRTUAL_TERMINAL_INPUT
        );
        // Raw mode, not harden, is what strips ECHO/LINE.
        assert_eq!(out & ENABLE_ECHO_INPUT, ENABLE_ECHO_INPUT);
        assert_eq!(out & ENABLE_LINE_INPUT, ENABLE_LINE_INPUT);
    }

    #[test]
    fn harden_input_from_raw_matches_observed_path() {
        // enable_raw_mode clears ECHO|LINE|PROCESSED (low 3 bits).
        let after_raw =
            COOKED_INPUT & !(ENABLE_ECHO_INPUT | ENABLE_LINE_INPUT | ENABLE_PROCESSED_INPUT);
        assert_eq!(after_raw, 0x1f0);
        let after_harden = harden_input_mode(after_raw);
        assert_eq!(after_harden & ENABLE_QUICK_EDIT_MODE, 0);
        assert_eq!(after_harden & ENABLE_EXTENDED_FLAGS, ENABLE_EXTENDED_FLAGS);
        assert_eq!(
            after_harden & ENABLE_VIRTUAL_TERMINAL_INPUT,
            ENABLE_VIRTUAL_TERMINAL_INPUT
        );
    }

    #[test]
    fn harden_output_sets_vt_processing() {
        let out = harden_output_mode(ENABLE_PROCESSED_OUTPUT | ENABLE_WRAP_AT_EOL_OUTPUT);
        assert_eq!(
            out & ENABLE_VIRTUAL_TERMINAL_PROCESSING,
            ENABLE_VIRTUAL_TERMINAL_PROCESSING,
            "without VT-OUT, CSI ?1049h is ignored and the shell prompt stays in the TUI"
        );
        assert_eq!(
            out & DISABLE_NEWLINE_AUTO_RETURN,
            DISABLE_NEWLINE_AUTO_RETURN
        );
    }

    #[test]
    fn harden_output_from_zero_still_enables_vt() {
        let out = harden_output_mode(0);
        assert_ne!(out & ENABLE_VIRTUAL_TERMINAL_PROCESSING, 0);
    }

    fn fake_pe(subsystem: u16) -> Vec<u8> {
        let mut b = vec![0u8; 0x100];
        b[0] = b'M';
        b[1] = b'Z';
        b[0x3c] = 0x40;
        b[0x40] = b'P';
        b[0x41] = b'E';
        let opt = 0x58;
        b[opt] = 0x0b;
        b[opt + 1] = 0x02; // PE32+
        b[opt + 68] = (subsystem & 0xff) as u8;
        b[opt + 69] = (subsystem >> 8) as u8;
        b
    }

    #[test]
    fn pe_subsystem_reads_cui_and_gui() {
        assert_eq!(
            pe_subsystem(&fake_pe(IMAGE_SUBSYSTEM_WINDOWS_CUI)),
            Some(IMAGE_SUBSYSTEM_WINDOWS_CUI)
        );
        assert_eq!(
            pe_subsystem(&fake_pe(IMAGE_SUBSYSTEM_WINDOWS_GUI)),
            Some(IMAGE_SUBSYSTEM_WINDOWS_GUI)
        );
        assert_eq!(pe_subsystem(b"not a pe"), None);
    }

    #[test]
    fn mutation_forgetting_vt_out_is_caught() {
        // If someone "simplifies" harden_output_mode to only set WRAP, this
        // must go red — that regression is exactly the shell-in-TUI bug.
        fn broken(mode: u32) -> u32 {
            mode | ENABLE_WRAP_AT_EOL_OUTPUT
        }
        assert_eq!(
            broken(0) & ENABLE_VIRTUAL_TERMINAL_PROCESSING,
            0,
            "sanity: the broken stand-in really omits VT-OUT"
        );
        assert_ne!(
            harden_output_mode(0) & ENABLE_VIRTUAL_TERMINAL_PROCESSING,
            broken(0) & ENABLE_VIRTUAL_TERMINAL_PROCESSING
        );
    }
}
