//! Headless terminal mirror on alacritty_terminal: visible rows for approval detection, and a serialized snapshot
//! (with scrollback) for a client attaching midway. Replays are wrong by nature: output was drawn at other widths.
use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::test::TermSize;
use alacritty_terminal::term::{Config, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor, StdSyncHandler};
use alacritty_terminal::Term;

pub struct Mirror {
    term: Term<VoidListener>,
    parser: Processor<StdSyncHandler>,
}

fn sgr_color(out: &mut Vec<String>, c: Color, base: u16, bright: u16, ext: u16) {
    match c {
        Color::Named(n) => {
            let v = n as usize;
            if v < 8 {
                out.push((base + v as u16).to_string());
            } else if v < 16 {
                out.push((bright + (v - 8) as u16).to_string());
            } else if let Some(dim) = dim_index(n) {
                out.push((base + dim).to_string());
            }
            // Foreground, Background, Cursor and the like are the defaults: nothing to emit
        }
        Color::Indexed(i) => out.push(format!("{ext};5;{i}")),
        Color::Spec(rgb) => out.push(format!("{ext};2;{};{};{}", rgb.r, rgb.g, rgb.b)),
    }
}

fn dim_index(n: NamedColor) -> Option<u16> {
    use NamedColor::*;
    Some(match n {
        DimBlack => 0,
        DimRed => 1,
        DimGreen => 2,
        DimYellow => 3,
        DimBlue => 4,
        DimMagenta => 5,
        DimCyan => 6,
        DimWhite => 7,
        _ => return None,
    })
}

fn sgr_of(cell: &Cell) -> String {
    let mut p = vec!["0".to_string()];
    let f = cell.flags;
    if f.contains(Flags::BOLD) {
        p.push("1".into());
    }
    if f.contains(Flags::DIM) {
        p.push("2".into());
    }
    if f.contains(Flags::ITALIC) {
        p.push("3".into());
    }
    if f.intersects(Flags::ALL_UNDERLINES) {
        p.push("4".into());
    }
    if f.contains(Flags::INVERSE) {
        p.push("7".into());
    }
    if f.contains(Flags::HIDDEN) {
        p.push("8".into());
    }
    if f.contains(Flags::STRIKEOUT) {
        p.push("9".into());
    }
    sgr_color(&mut p, cell.fg, 30, 90, 38);
    sgr_color(&mut p, cell.bg, 40, 100, 48);
    format!("\x1b[{}m", p.join(";"))
}

impl Mirror {
    pub fn new(cols: u16, rows: u16, scrollback: usize) -> Self {
        let size = TermSize::new(cols as usize, rows as usize);
        let config = Config { scrolling_history: scrollback, ..Config::default() };
        Self { term: Term::new(config, &size, VoidListener), parser: Processor::new() }
    }

    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.term.resize(TermSize::new(cols as usize, rows as usize));
    }

    pub fn size(&self) -> (u16, u16) {
        (self.term.columns() as u16, self.term.screen_lines() as u16)
    }

    fn line_text(&self, line: Line) -> String {
        let grid = self.term.grid();
        let row = &grid[line];
        let mut s = String::new();
        for x in 0..grid.columns() {
            let cell = &row[Column(x)];
            if cell.flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
                continue;
            }
            // alacritty keeps the tab character in the first cell it skips; on screen it is a space
            s.push(if cell.c == '\t' { ' ' } else { cell.c });
            if let Some(extra) = cell.zerowidth() {
                s.extend(extra.iter());
            }
        }
        s.truncate(s.trim_end_matches(' ').len());
        s
    }

    /// One string per visible row, the input of approval detection.
    pub fn rows(&self) -> Vec<String> {
        (0..self.term.screen_lines() as i32).map(|y| self.line_text(Line(y))).collect()
    }

    pub fn history_len(&self) -> usize {
        self.term.grid().history_size()
    }

    /// All lines, oldest first (scrollback then screen): lets tests compare the whole buffer.
    pub fn all_rows(&self) -> Vec<String> {
        let g = self.term.grid();
        (g.topmost_line().0..=g.bottommost_line().0).map(|y| self.line_text(Line(y))).collect()
    }

    /// Bytes that rebuild the screen and up to `scrollback` lines of history on a fresh terminal of the same size.
    /// ponytail: in the alternate screen only that screen is kept; the normal buffer is not serialized.
    pub fn serialize(&self, scrollback: usize) -> String {
        let g = self.term.grid();
        let alt = self.term.mode().contains(TermMode::ALT_SCREEN);
        let top = g.topmost_line().0.max(-(scrollback.min(g.history_size()) as i32));
        let bottom = g.bottommost_line().0;
        let cols = g.columns();
        let mut out = String::new();
        if alt {
            out.push_str("\x1b[?1049h\x1b[H");
        }
        let mut last_sgr = String::from("\x1b[0m");
        for y in top..=bottom {
            let row = &g[Line(y)];
            // The last cell that shows something; empties after it are just unwritten space.
            let mut end = 0;
            for x in 0..cols {
                let c = &row[Column(x)];
                let blank = (c.c == ' ' || c.c == '\t') && c.bg == Color::Named(NamedColor::Background) && !c.flags.intersects(Flags::INVERSE | Flags::ALL_UNDERLINES | Flags::STRIKEOUT);
                if !blank && !c.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
                    end = x + 1;
                }
            }
            for x in 0..end {
                let cell = &row[Column(x)];
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    continue;
                }
                let sgr = sgr_of(cell);
                if sgr != last_sgr {
                    out.push_str(&sgr);
                    last_sgr = sgr;
                }
                out.push(if cell.c == '\t' { ' ' } else { cell.c });
                if let Some(extra) = cell.zerowidth() {
                    out.extend(extra.iter());
                }
            }
            if y < bottom {
                // A soft wrap continues on the next line by itself: \r\n would break the line in two.
                if !row[Column(cols - 1)].flags.contains(Flags::WRAPLINE) {
                    if last_sgr != "\x1b[0m" {
                        out.push_str("\x1b[0m");
                        last_sgr = String::from("\x1b[0m");
                    }
                    out.push_str("\r\n");
                }
            }
        }
        if last_sgr != "\x1b[0m" {
            out.push_str("\x1b[0m");
        }
        let cursor = g.cursor.point;
        out.push_str(&format!("\x1b[{};{}H", cursor.line.0 + 1, cursor.column.0 + 1));
        if !self.term.mode().contains(TermMode::SHOW_CURSOR) {
            out.push_str("\x1b[?25l");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_and_wrap() {
        let mut m = Mirror::new(10, 3, 100);
        m.process(b"\x1b[32mhello\x1b[0m\r\n");
        m.process("0123456789abc".as_bytes());
        assert_eq!(m.rows(), vec!["hello", "0123456789", "abc"]);
    }

    #[test]
    fn serialize_round_trip_keeps_history_wrap_colour_and_cursor() {
        let mut a = Mirror::new(20, 4, 100);
        for i in 0..12 {
            a.process(format!("\x1b[31mline {i}\x1b[0m plain\r\n").as_bytes());
        }
        a.process("中文寬字元測試測試測試測試".as_bytes()); // wraps at a wide char
        a.process(b"\x1b[2;3H");
        let snap = a.serialize(100);
        let mut b = Mirror::new(20, 4, 100);
        b.process(snap.as_bytes());
        assert_eq!(a.all_rows(), b.all_rows());
        assert_eq!(a.rows(), b.rows());
        assert_eq!(a.term.grid().cursor.point, b.term.grid().cursor.point);
        assert!(snap.contains("\x1b[0;31m"), "colours survive: {snap:?}");
        assert_eq!(a.serialize(100), b.serialize(100), "stable when applied again");
    }

    #[test]
    fn serialize_limits_history() {
        let mut a = Mirror::new(10, 2, 100);
        for i in 0..50 {
            a.process(format!("{i}\r\n").as_bytes());
        }
        let mut b = Mirror::new(10, 2, 100);
        b.process(a.serialize(5).as_bytes());
        assert_eq!(b.history_len(), 5);
    }

    #[test]
    fn resize_reflows_instead_of_truncating() {
        let mut m = Mirror::new(20, 4, 100);
        m.process(b"abcdefghijklmnopqrst");
        m.resize(10, 4);
        m.resize(20, 4);
        assert!(m.all_rows().iter().any(|r| r.starts_with("abcdefghijklmnopqrst")), "{:?}", m.all_rows());
    }
}
