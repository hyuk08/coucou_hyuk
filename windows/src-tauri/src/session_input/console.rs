// The Win32 half of session_input.rs: typing into another process's console.
//
// Coucou has no console of its own (a GUI app), so it may attach to the
// session's for a moment. The text goes into that console's input buffer as
// key presses, exactly what the terminal writes there when the user types;
// Claude Code reads them like any other keys. Enter follows after a short
// pause, so the text is in the prompt before it is sent.

use std::time::Duration;

use windows::core::{w, BOOL, PWSTR};
use windows::Win32::Foundation::{CloseHandle, FILETIME, GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::Console::{
    AttachConsole, FreeConsole, SetConsoleCtrlHandler, WriteConsoleInputW, INPUT_RECORD, INPUT_RECORD_0,
    KEY_EVENT, KEY_EVENT_RECORD, KEY_EVENT_RECORD_0,
};
use windows::Win32::System::Threading::{
    GetExitCodeProcess, GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

const STILL_ACTIVE: u32 = 259;
const VK_RETURN: u16 = 0x0D;
const SCAN_RETURN: u16 = 0x1C;
/// Time for the prompt to take the text in before Enter sends it.
const BEFORE_ENTER: Duration = Duration::from_millis(300);

/// A running process's executable path and start time, or None once it is gone.
pub fn process_info(pid: u32) -> Option<(String, u64)> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let info = (|| {
            let mut code = 0u32;
            GetExitCodeProcess(process, &mut code).ok()?;
            if code != STILL_ACTIVE {
                return None;
            }
            let (mut created, mut exited, mut kernel, mut user) =
                (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
            GetProcessTimes(process, &mut created, &mut exited, &mut kernel, &mut user).ok()?;
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).ok()?;
            let exe = String::from_utf16_lossy(&buf[..len as usize]);
            Some((exe, ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64))
        })();
        let _ = CloseHandle(process);
        info
    }
}

fn key(down: bool, vk: u16, scan: u16, ch: u16) -> INPUT_RECORD {
    INPUT_RECORD {
        EventType: KEY_EVENT as u16,
        Event: INPUT_RECORD_0 {
            KeyEvent: KEY_EVENT_RECORD {
                bKeyDown: down.into(),
                wRepeatCount: 1,
                wVirtualKeyCode: vk,
                wVirtualScanCode: scan,
                uChar: KEY_EVENT_RECORD_0 { UnicodeChar: ch },
                dwControlKeyState: 0,
            },
        },
    }
}

/// The key presses that type `text`: each UTF-16 unit down then up.
pub fn text_records(text: &str) -> Vec<INPUT_RECORD> {
    text.encode_utf16().flat_map(|u| [key(true, 0, 0, u), key(false, 0, 0, u)]).collect()
}

fn enter_records() -> [INPUT_RECORD; 2] {
    let cr = '\r' as u16;
    [key(true, VK_RETURN, SCAN_RETURN, cr), key(false, VK_RETURN, SCAN_RETURN, cr)]
}

/// While attached, a Ctrl+C or Ctrl+Break in that terminal is not for Coucou.
unsafe extern "system" fn ignore_console_events(_kind: u32) -> BOOL {
    true.into()
}

/// Detaches and closes whatever was opened, however `type_line` ends.
struct Attached {
    input: Option<HANDLE>,
}

impl Drop for Attached {
    fn drop(&mut self) {
        unsafe {
            if let Some(h) = self.input.take() {
                let _ = CloseHandle(h);
            }
            let _ = FreeConsole();
            let _ = SetConsoleCtrlHandler(Some(ignore_console_events), false);
        }
    }
}

fn write_all(input: HANDLE, records: &[INPUT_RECORD]) -> Result<(), String> {
    for chunk in records.chunks(512) {
        let mut written = 0u32;
        unsafe { WriteConsoleInputW(input, chunk, &mut written) }.map_err(|e| e.message())?;
    }
    Ok(())
}

/// Types `text` into the console `pid` reads from, then presses Enter.
pub fn type_line(pid: u32, text: &str) -> Result<(), String> {
    type_into(pid, text, true)
}

pub fn type_into(pid: u32, text: &str, enter: bool) -> Result<(), String> {
    unsafe {
        // Never a console of our own (a debug build has one): only the session's.
        let _ = FreeConsole();
        let _ = SetConsoleCtrlHandler(Some(ignore_console_events), true);
        let mut attached = Attached { input: None };
        AttachConsole(pid).map_err(|_| "This session doesn't run in a terminal Coucou can type into.".to_string())?;
        let input = CreateFileW(
            w!("CONIN$"),
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
        .map_err(|e| e.message())?;
        attached.input = Some(input);
        write_all(input, &text_records(text))?;
        if enter {
            std::thread::sleep(BEFORE_ENTER);
            write_all(input, &enter_records())?;
        }
        Ok(())
    }
}
