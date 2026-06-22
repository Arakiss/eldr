//! Owned terminal engine: termios raw mode + ANSI primitives + a poll-based key
//! reader. No `ratatui`/`crossterm`. A [`RawMode`] guard restores the terminal on
//! drop (cursor, alternate screen, cooked mode), so quitting never leaks raw mode.

use core::ffi::c_int;
use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

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
    fn signal(signum: c_int, handler: extern "C" fn(c_int)) -> usize;
}

// macOS SIGWINCH (terminal resize).
const SIGWINCH: c_int = 28;

// Cached terminal size + a "needs (re)query" flag, for the cursor-report fallback used
// when the winsize ioctl is unreliable. A SIGWINCH marks the cache stale so a resize is
// picked up without round-tripping the terminal every frame.
static SIZE_CACHE: AtomicU32 = AtomicU32::new(0);
static NEED_QUERY: AtomicBool = AtomicBool::new(true);

extern "C" fn on_winch(_sig: c_int) {
    NEED_QUERY.store(true, Ordering::SeqCst);
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
        // Watch for resizes so the cursor-report size fallback re-queries; force a query
        // on the next size() call.
        unsafe { signal(SIGWINCH, on_winch) };
        NEED_QUERY.store(true, Ordering::SeqCst);
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
/// override (pins a size; a manual escape hatch); `TIOCGWINSZ` on stdout, stdin, then
/// stderr; the controlling terminal `/dev/tty` directly (when 0/1/2 are redirected); a
/// cursor-position report that asks the terminal itself (for multiplexers whose pty
/// winsize is stale even though the window is bigger); the shell's exported
/// `COLUMNS`/`LINES`; and finally `(80, 24)`. The cursor-report path is what makes it
/// auto-fill inside a surface whose `TIOCGWINSZ` lies.
pub fn size() -> (u16, u16) {
    if let (Some(c), Some(r)) = (env_u16("ELDR_COLS"), env_u16("ELDR_ROWS")) {
        return (c, r);
    }
    // Ask the terminal itself first (when in raw mode on a tty): the cursor-position
    // report measures the actual drawable area, which is correct even when a multiplexer
    // surface's pty winsize is stale. Cached until the next SIGWINCH so it doesn't
    // round-trip every frame. Falls through when the terminal doesn't answer DSR.
    if let Some(sz) = cached_cursor_size() {
        return sz;
    }
    // The kernel's winsize, for plain terminals / non-raw contexts.
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

fn pack(c: u16, r: u16) -> u32 {
    ((c as u32) << 16) | r as u32
}
fn unpack(v: u32) -> Option<(u16, u16)> {
    if v == 0 {
        None
    } else {
        Some(((v >> 16) as u16, (v & 0xffff) as u16))
    }
}

/// The cursor-report size, queried at most once per resize and cached; falls back to the
/// last good value if a query fails.
fn cached_cursor_size() -> Option<(u16, u16)> {
    // Until the next SIGWINCH, reuse whatever the last attempt produced (Some or None) —
    // never re-query per frame, which would stall the loop and eat keystrokes if the
    // terminal doesn't answer DSR.
    if !NEED_QUERY.load(Ordering::SeqCst) {
        return unpack(SIZE_CACHE.load(Ordering::SeqCst));
    }
    NEED_QUERY.store(false, Ordering::SeqCst);
    if let Some((c, r)) = size_via_cursor() {
        SIZE_CACHE.store(pack(c, r), Ordering::SeqCst);
        Some((c, r))
    } else {
        unpack(SIZE_CACHE.load(Ordering::SeqCst))
    }
}

/// Ask the terminal for its size by parking the cursor far past the edge and reading the
/// cursor-position report (`ESC [ rows ; cols R`). Works through multiplexers that relay
/// DSR even when the pty winsize is stale. No-ops when stdout isn't a terminal.
fn size_via_cursor() -> Option<(u16, u16)> {
    if !std::io::stdout().is_terminal() {
        return None;
    }
    {
        let mut out = std::io::stdout();
        out.write_all(b"\x1b[s\x1b[999;999H\x1b[6n\x1b[u").ok()?;
        out.flush().ok()?;
    }
    let mut buf: Vec<u8> = Vec::with_capacity(16);
    loop {
        let mut pfd = Pollfd {
            fd: 0,
            events: POLLIN,
            revents: 0,
        };
        let rc = unsafe { poll(&mut pfd, 1, 60) };
        if rc <= 0 || (pfd.revents & POLLIN) == 0 {
            break;
        }
        let mut byte = 0u8;
        if unsafe { read(0, &mut byte, 1) } != 1 {
            break;
        }
        buf.push(byte);
        if byte == b'R' || buf.len() > 32 {
            break;
        }
    }
    parse_cursor(&buf)
}

/// Parse `ESC [ rows ; cols R` → `(cols, rows)`.
fn parse_cursor(buf: &[u8]) -> Option<(u16, u16)> {
    let r = buf.iter().rposition(|&b| b == b'R')?;
    let lb = buf[..r].iter().rposition(|&b| b == b'[')?;
    let (rows, cols) = std::str::from_utf8(&buf[lb + 1..r]).ok()?.split_once(';')?;
    let rows: u16 = rows.trim().parse().ok()?;
    let cols: u16 = cols.trim().parse().ok()?;
    (cols > 0 && rows > 0).then_some((cols, rows))
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

    #[test]
    fn parses_cursor_report() {
        // ESC [ rows ; cols R  ->  (cols, rows)
        assert_eq!(super::parse_cursor(b"\x1b[29;229R"), Some((229, 29)));
        // Tolerates leading noise before the report.
        assert_eq!(super::parse_cursor(b"x\x1b[24;80R"), Some((80, 24)));
        // Garbage / incomplete -> None.
        assert_eq!(super::parse_cursor(b"\x1b[29;"), None);
        assert_eq!(super::parse_cursor(b"nope"), None);
        assert_eq!(super::parse_cursor(b"\x1b[0;0R"), None);
    }
}
