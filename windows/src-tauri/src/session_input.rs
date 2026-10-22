// "Send to Claude Code": the island's text goes to a Claude Code session as if
// it had been typed in its terminal, and Enter sends it (Windows).
//
// When a session's relay connects, the walk up its process tree stops at the
// first claude.exe (or node.exe running Claude Code); that process and its
// start time are kept per session, in memory, like session_window.rs. Sending
// attaches Coucou to that process's console for a moment and writes the text
// as key presses into its input buffer (console.rs). No window is brought
// forward and nothing is pasted anywhere else: only that console gets the keys,
// so an editor that happens to have the focus can never receive them.
//
// Pasted images ride along as file paths: a screenshot Windows has just saved
// is sent where it is, any other image is first written to the inbox.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[cfg(windows)]
pub mod console;

const MAX_SESSIONS: usize = 64;
/// Longer than anything said or typed in the island, short enough to type at once.
pub const MAX_TEXT: usize = 8000;
const MAX_IMAGE_BYTES: usize = 25 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Target {
    pid: u32,
    started: u64,
}

/// Session → its Claude Code process, or None once looked for and not found.
static SESSIONS: Mutex<Vec<(String, Option<Target>)>> = Mutex::new(Vec::new());
/// Attaching to a console is for the whole process: one send at a time.
static SENDING: Mutex<()> = Mutex::new(());

fn valid_session(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A hook from `session` came through the relay `relay_pid`: find its Claude
/// Code process once.
pub fn note(session: &str, relay_pid: u32) {
    if !valid_session(session) {
        return;
    }
    {
        let list = SESSIONS.lock().unwrap_or_else(|e| e.into_inner());
        if list.iter().any(|(s, _)| s == session) {
            return;
        }
    }
    let target = find_target(relay_pid);
    // No ancestors at all: the relay was already gone, so try again next time.
    #[cfg(windows)]
    if target.is_none() && crate::platform::process_ancestors(relay_pid).is_empty() {
        return;
    }
    let mut list = SESSIONS.lock().unwrap_or_else(|e| e.into_inner());
    list.retain(|(s, _)| s != session);
    list.push((session.to_string(), target));
    let excess = list.len().saturating_sub(MAX_SESSIONS);
    list.drain(..excess);
}

pub fn forget(session: &str) {
    SESSIONS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(s, _)| s != session);
}

#[cfg(windows)]
fn find_target(relay_pid: u32) -> Option<Target> {
    crate::platform::process_ancestors(relay_pid).into_iter().find_map(|pid| {
        let (exe, started) = console::process_info(pid)?;
        is_claude(&exe).then_some(Target { pid, started })
    })
}

#[cfg(not(windows))]
fn find_target(_relay_pid: u32) -> Option<Target> {
    None
}

/// Claude Code's own executable, or the Node.js an npm install runs it on.
fn is_claude(exe_path: &str) -> bool {
    let name = exe_path.rsplit(['\\', '/']).next().unwrap_or(exe_path);
    name.eq_ignore_ascii_case("claude.exe") || name.eq_ignore_ascii_case("node.exe")
}

/// What is typed: one line, no control characters (no Escape, no Ctrl+…, no
/// Enter in the middle), so the text can only ever be text.
pub fn clean_text(text: &str) -> String {
    let one_line: String = text
        .chars()
        .map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect();
    let joined = one_line.split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    joined.chars().take(MAX_TEXT).collect()
}

/// The message as Claude Code gets it: the text, then each image's path in quotes.
pub fn compose(text: &str, images: &[String]) -> String {
    let mut out = clean_text(text);
    for path in images {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push('"');
        out.push_str(&clean_text(path));
        out.push('"');
    }
    out
}

pub fn send(session: &str, text: &str, images: &[String]) -> Result<(), String> {
    let message = compose(text, images);
    if message.is_empty() {
        return Err("Nothing to send.".into());
    }
    let target = {
        let list = SESSIONS.lock().unwrap_or_else(|e| e.into_inner());
        list.iter().find(|(s, _)| s == session).and_then(|(_, t)| *t)
    };
    let Some(target) = target else {
        return Err("This session doesn't run in a terminal Coucou can type into.".into());
    };
    send_to(target, &message)
}

#[cfg(windows)]
fn send_to(target: Target, message: &str) -> Result<(), String> {
    // The same process, still running: a reused process ID never gets the text.
    match console::process_info(target.pid) {
        Some((exe, started)) if started == target.started && is_claude(&exe) => {}
        _ => return Err("This Claude Code session has ended.".into()),
    }
    let _one = SENDING.lock().unwrap_or_else(|e| e.into_inner());
    console::type_line(target.pid, message)
}

#[cfg(not(windows))]
fn send_to(_target: Target, _message: &str) -> Result<(), String> {
    Err("Sending to Claude Code is only on Windows for now.".into())
}

// ── Pasted images ────────────────────────────────────────────────────────────

/// Width and height from a PNG's header, without decoding it.
pub fn png_size(head: &[u8]) -> Option<(u32, u32)> {
    if head.len() < 24 || &head[..8] != b"\x89PNG\r\n\x1a\n" || &head[12..16] != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(head[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(head[20..24].try_into().ok()?);
    Some((w, h))
}

/// A screenshot Windows saved in the last few minutes with these dimensions:
/// the image just pasted, already on disk.
pub fn recent_screenshot(dir: &Path, width: u32, height: u32) -> Option<PathBuf> {
    use std::io::Read;
    let now = std::time::SystemTime::now();
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("png")))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .filter(|(m, _)| now.duration_since(*m).map_or(true, |age| age.as_secs() <= 300))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    files.into_iter().take(5).map(|(_, p)| p).find(|p| {
        let mut head = [0u8; 24];
        std::fs::File::open(p).and_then(|mut f| f.read_exact(&mut head)).is_ok() && png_size(&head) == Some((width, height))
    })
}

/// Any other pasted image: written to the inbox, named by the time.
pub fn save_image(dir: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
    if bytes.is_empty() || bytes.len() > MAX_IMAGE_BYTES {
        return Err("The image is too large.".into());
    }
    let ext = if png_size(bytes).is_some() { "png" } else if bytes.starts_with(b"\xff\xd8") { "jpg" } else {
        return Err("Only PNG and JPEG images can be sent.".into());
    };
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let path = dir.join(format!("pasted-{stamp}.{ext}"));
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_is_one_line_without_control_characters() {
        assert_eq!(clean_text("  hello\r\nworld\t!\x1b[A\x03 "), "hello world ![A");
        assert_eq!(clean_text("안녕하세요?"), "안녕하세요?");
        assert_eq!(clean_text(&"a".repeat(MAX_TEXT + 10)).len(), MAX_TEXT);
    }

    #[test]
    fn images_follow_the_text_as_quoted_paths() {
        let images = vec![r"C:\Users\Han S\Pictures\Screenshots\a.png".to_string()];
        assert_eq!(compose("이거 봐줘", &images), r#"이거 봐줘 "C:\Users\Han S\Pictures\Screenshots\a.png""#);
        assert_eq!(compose("", &images), r#""C:\Users\Han S\Pictures\Screenshots\a.png""#);
        assert_eq!(compose(" ", &[]), "");
    }

    #[test]
    fn only_claude_code_is_typed_into() {
        assert!(is_claude(r"C:\Users\x\AppData\Roaming\npm\node_modules\@anthropic-ai\claude-code\bin\claude.exe"));
        assert!(is_claude("node.exe"));
        assert!(!is_claude(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"));
        assert!(!is_claude("cmd.exe"));
    }

    #[test]
    fn an_unknown_session_is_never_typed_into() {
        assert!(send("no-such-session", "hi", &[]).is_err());
        assert!(send("bad id!", "hi", &[]).is_err());
    }

    #[test]
    fn png_dimensions_come_from_the_header() {
        let mut head = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        head.extend_from_slice(&1920u32.to_be_bytes());
        head.extend_from_slice(&1080u32.to_be_bytes());
        assert_eq!(png_size(&head), Some((1920, 1080)));
        assert_eq!(png_size(b"GIF89a"), None);
    }

    #[test]
    fn a_recent_screenshot_of_the_same_size_is_found() {
        let dir = std::env::temp_dir().join(format!("coucou-shots-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&800u32.to_be_bytes());
        png.extend_from_slice(&600u32.to_be_bytes());
        std::fs::write(dir.join("shot.png"), &png).unwrap();
        assert_eq!(recent_screenshot(&dir, 800, 600), Some(dir.join("shot.png")));
        assert_eq!(recent_screenshot(&dir, 801, 600), None);
        let saved = save_image(&dir, &png).unwrap();
        assert!(saved.extension().is_some_and(|x| x == "png"));
        assert!(save_image(&dir, b"not an image").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
