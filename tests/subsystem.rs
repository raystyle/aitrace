//! The aitrace binary must be a Windows *console* subsystem image.
//! GUI subsystem (`windows_subsystem = "windows"`) lets pwsh/cmd return to
//! the prompt immediately; the shell then keeps echo and the process never
//! sees keyboard input.

#[cfg(windows)]
#[test]
fn aitrace_exe_is_console_subsystem() {
    const IMAGE_SUBSYSTEM_WINDOWS_CUI: u16 = 3;

    let path = env!("CARGO_BIN_EXE_aitrace");
    let sub = pe_subsystem(std::path::Path::new(path)).expect("parse PE subsystem of aitrace.exe");
    assert_eq!(
        sub, IMAGE_SUBSYSTEM_WINDOWS_CUI,
        "aitrace.exe must be IMAGE_SUBSYSTEM_WINDOWS_CUI (3), not GUI (2); \
         GUI lets the parent shell keep the prompt and steal keyboard input"
    );
}

/// Read the `Subsystem` field from a PE file's optional header.
#[cfg(windows)]
fn pe_subsystem(path: &std::path::Path) -> std::io::Result<u16> {
    use std::io::{Read, Seek, SeekFrom};

    let mut f = std::fs::File::open(path)?;
    // DOS header: e_lfanew at offset 0x3c points at the PE signature.
    let mut dos = [0u8; 64];
    f.read_exact(&mut dos)?;
    let pe_off = u32::from_le_bytes([dos[60], dos[61], dos[62], dos[63]]) as u64;

    f.seek(SeekFrom::Start(pe_off))?;
    let mut sig = [0u8; 4];
    f.read_exact(&mut sig)?;
    if &sig != b"PE\0\0" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "not a PE file",
        ));
    }

    // COFF header (20 bytes) follows the signature; the optional header
    // after it. Subsystem sits at optional-header offset 68.
    f.seek(SeekFrom::Current(20 + 68))?;
    let mut sub = [0u8; 2];
    f.read_exact(&mut sub)?;
    Ok(u16::from_le_bytes(sub))
}
