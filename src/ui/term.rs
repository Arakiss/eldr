//! Owned terminal engine: termios raw mode + ANSI primitives + a poll-based key
//! reader. No `ratatui`/`crossterm`. A [`RawMode`] guard restores the terminal on
//! drop (cursor, alternate screen, cooked mode), so quitting never leaks raw mode.

use core::ffi::c_int;
use std::io::Write;

// macOS termios constants.
const ICANON: u64 = 0x0000_0100;
const ECHO: u64 = 0x0000_0008;
const ISIG: u64 = 0x0000_0080;
const IEXTEN: u64 = 0x0000_0400;
const IXON: u64 = 0x0000_0200;
const ICRNL: u64 = 0x0000_0100;
const BRKINT: u64 = 0x0000_0002;
const INPCK: u64 = 0x0000_0010;
const ISTRIP: u64 = 0x0000_0020;
const VMIN: usize = 16;
const VTIME: usize = 17;
const TCSANOW: c_int = 0;
const NCCS: usize = 20;
const POLLIN: i16 = 0x0001;
// macOS `_IOR('t', 104, struct winsize)`.
const TIOCGWINSZ: u64 = 0x4008_7468;

#[repr(C)]
#[derive(Clone, Copy)]
struct Termios {
    c_iflag: u64,
    c_oflag: u64,
    c_cflag: u64,
    c_lflag: u64,
    c_cc: [u8; NCCS],
    c_ispeed: u64,
    c_ospeed: u64,
}

#[repr(C)]
struct Pollfd {
    fd: c_int,
    events: i16,
    revents: i16,
}

#[repr(C)]
struct Winsize {
    row: u16,
    col: u16,
    xpixel: u16,
    ypixel: u16,
}

unsafe extern "C" {
    fn tcgetattr(fd: c_int, termios: *mut Termios) -> c_int;
    fn tcsetattr(fd: c_int, optional_actions: c_int, termios: *const Termios) -> c_int;
    fn poll(fds: *mut Pollfd, nfds: u32, timeout: c_int) -> c_int;
    fn read(fd: c_int, buf: *mut u8, count: usize) -> isize;
    fn ioctl(fd: c_int, request: u64, arg: *mut Winsize) -> c_int;
    fn open(path: *const core::ffi::c_char, flags: c_int) -> c_int;
    fn close(fd: c_int) -> c_int;
}

/// RAII raw-mode guard. Enter switches to raw mode + alternate screen + hidden
/// cursor; drop restores everything.
pub struct RawMode {
    orig: Termios,
    fd: c_int,
}

impl RawMode {
    pub fn enter() -> Option<Self> {
        let fd = 0;
        let mut orig = unsafe { std::mem::zeroed::<Termios>() };
        if unsafe { tcgetattr(fd, &mut orig) } != 0 {
            return None;
        }
        let mut raw = orig;
        // Clearing ISIG makes Ctrl-C arrive as byte 0x03 (handled in the loop), so we
        // restore the terminal ourselves rather than racing a signal handler.
        raw.c_lflag &= !(ICANON | ECHO | ISIG | IEXTEN);
        raw.c_iflag &= !(IXON | ICRNL | BRKINT | INPCK | ISTRIP);
        raw.c_cc[VMIN] = 0;
        raw.c_cc[VTIME] = 0;
        if unsafe { tcsetattr(fd, TCSANOW, &raw) } != 0 {
            return None;
        }
        // Alternate screen + hide cursor.
        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x1b[?1049h\x1b[?25l");
        let _ = out.flush();
        Some(RawMode { orig, fd })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let mut out = std::io::stdout();
        // Show cursor + leave alternate screen.
        let _ = out.write_all(b"\x1b[?25h\x1b[?1049l");
        let _ = out.flush();
        unsafe { tcsetattr(self.fd, TCSANOW, &self.orig) };
    }
}

/// Wait up to `timeout_ms` for a key on stdin. Returns the byte, or `None` on
/// timeout. `Some(0x03)` is Ctrl-C.
pub fn read_key(timeout_ms: i32) -> Option<u8> {
    let mut pfd = Pollfd {
        fd: 0,
        events: POLLIN,
        revents: 0,
    };
    let rc = unsafe { poll(&mut pfd, 1, timeout_ms) };
    if rc <= 0 || (pfd.revents & POLLIN) == 0 {
        return None;
    }
    let mut byte = 0u8;
    let n = unsafe { read(0, &mut byte, 1) };
    if n == 1 { Some(byte) } else { None }
}

/// Terminal size as `(cols, rows)`. Tries, in order: an explicit `ELDR_COLS`/`ELDR_ROWS`
/// override (a workaround for terminals/multiplexers whose `TIOCGWINSZ` misreports, and a
/// way to pin a size); `TIOCGWINSZ` on stdout, stdin, then stderr; the controlling
/// terminal `/dev/tty` directly (in case 0/1/2 are redirected); the shell's exported
/// `COLUMNS`/`LINES`; and finally `(80, 24)`. The multi-fd + `/dev/tty` path matters
/// because some setups answer the ioctl on one descriptor but not stdout.
pub fn size() -> (u16, u16) {
    if let (Some(c), Some(r)) = (env_u16("ELDR_COLS"), env_u16("ELDR_ROWS")) {
        return (c, r);
    }
    if let Some(sz) = ioctl_size(1)
        .or_else(|| ioctl_size(0))
        .or_else(|| ioctl_size(2))
    {
        return sz;
    }
    if let Some(sz) = tty_size() {
        return sz;
    }
    if let (Some(c), Some(r)) = (env_u16("COLUMNS"), env_u16("LINES")) {
        return (c, r);
    }
    (80, 24)
}

/// `TIOCGWINSZ` on one descriptor, `None` unless it answers with a real size.
fn ioctl_size(fd: c_int) -> Option<(u16, u16)> {
    let mut ws = Winsize {
        row: 0,
        col: 0,
        xpixel: 0,
        ypixel: 0,
    };
    let rc = unsafe { ioctl(fd, TIOCGWINSZ, &mut ws) };
    if rc == 0 && ws.col > 0 && ws.row > 0 {
        Some((ws.col, ws.row))
    } else {
        None
    }
}

/// Query the controlling terminal directly, for when 0/1/2 are pipes/redirected.
fn tty_size() -> Option<(u16, u16)> {
    let fd = unsafe {
        open(c"/dev/tty".as_ptr(), 0 /* O_RDONLY */)
    };
    if fd < 0 {
        return None;
    }
    let sz = ioctl_size(fd);
    unsafe { close(fd) };
    sz
}

fn env_u16(key: &str) -> Option<u16> {
    std::env::var(key)
        .ok()?
        .trim()
        .parse::<u16>()
        .ok()
        .filter(|&v| v > 0)
}

/// Move the cursor home (top-left) without clearing — frames overwrite in place.
pub fn home() -> &'static str {
    "\x1b[H"
}

/// Clear from cursor to end of line (kills stale trailing glyphs).
pub fn clear_eol() -> &'static str {
    "\x1b[K"
}

/// Clear from cursor to end of screen.
pub fn clear_eos() -> &'static str {
    "\x1b[J"
}

#[cfg(test)]
mod size_tests {
    #[test]
    fn override_wins() {
        // SAFETY: single-threaded test process.
        unsafe {
            std::env::set_var("ELDR_COLS", "220");
            std::env::set_var("ELDR_ROWS", "28");
        }
        assert_eq!(super::size(), (220, 28));
        unsafe {
            std::env::remove_var("ELDR_COLS");
            std::env::remove_var("ELDR_ROWS");
        }
    }
}
