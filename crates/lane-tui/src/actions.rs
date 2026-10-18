//! What a key can do on this machine, each only when the user presses it: put text on the
//! clipboard, open a page in the system's settings app, or open a lane's app in the browser.
//! None runs a command the user didn't ask for or touches the network.

use std::io::Write;
use std::process::{Command, Stdio};

/// Put `text` on the clipboard. True if it got there (or, with no clipboard tool, if the terminal
/// was asked to take it).
pub fn copy(text: &str) -> bool {
    if cfg!(windows) {
        // clip.exe reads UTF-16 with a byte-order mark, so paths with any characters survive.
        let mut wide = vec![0xFF, 0xFE];
        wide.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
        pipe("clip", &[], &wide)
    } else if cfg!(target_os = "macos") {
        pipe("pbcopy", &[], text.as_bytes())
    } else {
        pipe("wl-copy", &[], text.as_bytes())
            || pipe("xclip", &["-selection", "clipboard"], text.as_bytes())
            || pipe("xsel", &["--clipboard", "--input"], text.as_bytes())
            || osc52(text)
    }
}

/// Open a settings page such as `ms-settings:disksandvolumes`. True if the settings app started.
pub fn open(uri: &str) -> bool {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        return false;
    };
    Command::new(program)
        .arg(uri)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

/// Open a lane's app, like `http://fix-auth.localhost:4100`, in the default browser. True if the
/// browser was asked to.
pub fn open_url(url: &str) -> bool {
    if cfg!(windows) || cfg!(target_os = "macos") {
        return open(url);
    }
    Command::new("xdg-open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

fn pipe(program: &str, args: &[&str], bytes: &[u8]) -> bool {
    let Ok(mut child) = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let wrote = child
        .stdin
        .take()
        .is_some_and(|mut stdin| stdin.write_all(bytes).is_ok());
    child.wait().is_ok_and(|s| s.success()) && wrote
}

/// Ask the terminal to set the clipboard (OSC 52). Works in most modern terminals, and over SSH.
fn osc52(text: &str) -> bool {
    let mut out = std::io::stdout();
    write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes())).is_ok() && out.flush().is_ok()
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                s.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(super::base64(b""), "");
        assert_eq!(super::base64(b"f"), "Zg==");
        assert_eq!(super::base64(b"fo"), "Zm8=");
        assert_eq!(super::base64(b"foo"), "Zm9v");
        assert_eq!(super::base64(b"git clone"), "Z2l0IGNsb25l");
    }
}
