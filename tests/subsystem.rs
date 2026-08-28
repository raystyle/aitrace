//! The interactive binary must be a Windows *console* subsystem image.
//! GUI subsystem (`windows_subsystem = "windows"`) lets pwsh/cmd return to
//! the prompt immediately; the shell then keeps echo and the TUI never sees keys.

#[cfg(windows)]
#[test]
fn aitrace_exe_is_console_subsystem() {
    let path = env!("CARGO_BIN_EXE_aitrace");
    let sub = aitrace::tui::input_test::pe_subsystem_file(std::path::Path::new(path))
        .expect("parse PE subsystem of aitrace.exe");
    assert_eq!(
        sub,
        aitrace::tui::input_test::IMAGE_SUBSYSTEM_WINDOWS_CUI,
        "aitrace.exe must be IMAGE_SUBSYSTEM_WINDOWS_CUI (3), not GUI (2); \
         GUI lets the parent shell keep the prompt and steal keyboard input"
    );
}
