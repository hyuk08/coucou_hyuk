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
// Pasted images ride along as file paths: each is written to the inbox as it
// was pasted (a screenshot with what was drawn on it), and swept after a week.

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
/// Code process once. The relay names it (`claimed`), as it has usually exited
/// by now; its own process tree is the fallback, for an older relay.
pub fn note(session: &str, relay_pid: u32, claimed: Option<u32>) {
    if !valid_session(session) {
        return;
    }
    {
        let list = SESSIONS.lock().unwrap_or_else(|e| e.into_inner());
        if list.iter().any(|(s, _)| s == session) {
            return;
        }
    }
    let target = claimed.and_then(claimed_target).or_else(|| find_target(relay_pid));
    // No ancestors at all: the relay was already gone, so try again next time.
    #[cfg(windows)]
    if target.is_none() && crate::platform::process_ancestors(relay_pid).is_empty() {
        return;
    }
    let short: String = session.chars().take(8).collect();
    match target {
        Some(t) => crate::log::line(format!("send: session {short} types into process {}", t.pid)),
        None => crate::log::line(format!("send: session {short} has no Claude Code console among {}", chain(relay_pid))),
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

/// The process the relay named, if it really is Claude Code and still running.
#[cfg(windows)]
fn claimed_target(pid: u32) -> Option<Target> {
    let (exe, started) = console::process_info(pid)?;
    is_claude(&exe).then_some(Target { pid, started })
}

#[cfg(not(windows))]
fn claimed_target(_pid: u32) -> Option<Target> {
    None
}

/// The relay's ancestors by executable name, for the log.
#[cfg(windows)]
fn chain(relay_pid: u32) -> String {
    let names: Vec<String> = crate::platform::process_ancestors(relay_pid)
        .into_iter()
        .map(|pid| match console::process_info(pid) {
            Some((exe, _)) => exe.rsplit(['\\', '/']).next().unwrap_or_default().to_string(),
            None => format!("{pid}?"),
        })
        .collect();
    names.join(" < ")
}

#[cfg(not(windows))]
fn chain(_relay_pid: u32) -> String {
    String::new()
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
        let short: String = session.chars().take(8).collect();
        crate::log::line(format!("send: no console known for session {short}"));
        return Err("Coucou hasn't found this session's terminal yet. Send one message in the terminal, then try again.".into());
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
    let sent = console::type_line(target.pid, message);
    if let Err(e) = &sent {
        crate::log::line(format!("send: typing into process {} failed: {e}", target.pid));
    }
    sent
}

#[cfg(not(windows))]
fn send_to(_target: Target, _message: &str) -> Result<(), String> {
    Err("Sending to Claude Code is only on Windows for now.".into())
}

// ── The turn's messages ──────────────────────────────────────────────────────

/// The most read from the end of a transcript to find the turn's start.
const MAX_TAIL: u64 = 32 * 1024 * 1024;
const MAX_MESSAGES: usize = 30;

/// A Claude Code transcript, and nothing else: a .jsonl under ~/.claude/projects.
pub fn is_transcript(path: &Path, home: &Path) -> bool {
    let root = home.join(".claude").join("projects");
    path.extension().is_some_and(|x| x == "jsonl")
        && path.components().all(|c| !matches!(c, std::path::Component::ParentDir))
        && path.starts_with(&root)
}

/// Every text Claude wrote in the last turn, in order — what it said between
/// its tool calls as well as its final answer. The turn starts at the last
/// prompt: a user entry with text rather than tool results.
pub fn turn_messages(path: &Path) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else { return Vec::new() };
    let size = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut tail = 1024 * 1024u64;
    loop {
        let start = size.saturating_sub(tail);
        let mut buf = Vec::new();
        if file.seek(SeekFrom::Start(start)).is_err() || file.read_to_end(&mut buf).is_err() {
            return Vec::new();
        }
        let text = String::from_utf8_lossy(&buf);
        // A tail that starts mid-line drops that first, partial line.
        let lines: Vec<&str> = if start == 0 { text.lines().collect() } else { text.lines().skip(1).collect() };
        let (messages, found) = messages_since_prompt(&lines);
        if found || start == 0 || tail >= MAX_TAIL {
            return messages;
        }
        tail *= 4;
    }
}

fn messages_since_prompt(lines: &[&str]) -> (Vec<String>, bool) {
    let mut out: Vec<String> = Vec::new();
    for line in lines.iter().rev() {
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        let content = &entry["message"]["content"];
        match entry["type"].as_str() {
            Some("assistant") => {
                for block in content.as_array().into_iter().flatten().rev() {
                    if block["type"] == "text" {
                        let t = block["text"].as_str().unwrap_or_default().trim();
                        if !t.is_empty() && out.last().map(String::as_str) != Some(t) {
                            out.push(t.to_string());
                        }
                    }
                }
            }
            Some("user") => {
                let prompt = content.is_string()
                    || content.as_array().is_some_and(|blocks| blocks.iter().all(|b| b["type"] != "tool_result"));
                if prompt {
                    out.reverse();
                    out.truncate(MAX_MESSAGES);
                    return (out, true);
                }
            }
            _ => {}
        }
    }
    out.reverse();
    out.truncate(MAX_MESSAGES);
    (out, false)
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

/// A pasted image, written to the inbox as it was pasted, named by the time.
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
    fn the_turn_is_every_text_since_the_last_prompt() {
        let lines = [
            r#"{"type":"user","message":{"role":"user","content":"earlier question"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"old answer"}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":"how much CPU?"}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"..."}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Let me measure."}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash"}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","content":"1%"}]}}"#,
            r#"{"type":"attachment"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Almost nothing."}]}}"#,
        ];
        let (messages, found) = messages_since_prompt(&lines);
        assert!(found);
        assert_eq!(messages, vec!["Let me measure.", "Almost nothing."]);
    }

    #[test]
    fn only_claude_code_transcripts_are_read() {
        let home = Path::new(r"C:\Users\a");
        assert!(is_transcript(Path::new(r"C:\Users\a\.claude\projects\p\s.jsonl"), home));
        assert!(!is_transcript(Path::new(r"C:\Users\a\.claude\projects\..\..\secret.jsonl"), home));
        assert!(!is_transcript(Path::new(r"C:\Users\a\.claude\projects\p\s.txt"), home));
        assert!(!is_transcript(Path::new(r"C:\Users\a\Documents\s.jsonl"), home));
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
    fn a_pasted_image_is_saved_as_png_or_jpeg_only() {
        let dir = std::env::temp_dir().join(format!("coucou-paste-{}", std::process::id()));
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&800u32.to_be_bytes());
        png.extend_from_slice(&600u32.to_be_bytes());
        let saved = save_image(&dir, &png).unwrap();
        assert!(saved.extension().is_some_and(|x| x == "png"));
        assert_eq!(std::fs::read(&saved).unwrap(), png);
        assert!(save_image(&dir, b"not an image").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
