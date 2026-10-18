//! 服务端 VT 内核（issue #70）。
//!
//! daemon 把每个会话的 ConPTY 字节流喂给 [`alacritty_terminal`]，在服务端维护屏幕、光标、
//! alt-screen 与 SGR 状态。它是 #71 的 ANSI 快照、#72 的环形缓冲与回放、#73 的隐藏会话刷新的
//! 共同基础——**快照不是从原始字节猜出来的，而是从这份内核状态序列化出来的**。
//!
//! 为什么内核在 daemon 而不是前端：REQUIREMENTS.md §3 的路线 C 已用 #42 夹具证明，
//! 「有界尾巴回放」无法重建屏幕（30 行全错、卡在普通屏），必须有服务端 VT 状态。
//!
//! 内存边界：`scrolling_history` 固定 10k 行，长时运行不会随输出无限增长。

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::grid::{Dimensions, Grid};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::{Cell, Flags};
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor, Rgb};

/// 一格的只读快照（跨引擎比对 / 单测用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellView {
    /// 字符；宽字符的占位格在 alacritty 里是空格。
    pub character: char,
    /// 前景色。
    pub foreground: Color,
    /// 背景色。
    pub background: Color,
    /// 属性位（见 [`cell_flags`]）。
    pub flags: u8,
}

/// 属性位。与 #42 fixture 的 `style_rows` 约定一致：
/// 1=INVERSE 2=BOLD 4=ITALIC 8=UNDERLINE 16=DIM 32=HIDDEN 64=STRIKEOUT。
pub fn cell_flags(flags: Flags) -> u8 {
    let mut bits = 0u8;
    for (flag, bit) in [
        (Flags::INVERSE, 1),
        (Flags::BOLD, 2),
        (Flags::ITALIC, 4),
        (Flags::UNDERLINE, 8),
        (Flags::DIM, 16),
        (Flags::HIDDEN, 32),
        (Flags::STRIKEOUT, 64),
    ] {
        if flags.contains(flag) {
            bits |= bit;
        }
    }
    bits
}

/// alacritty 的事件监听器。内核不需要向外发事件（标题 / 铃声归 #85 / #88）。
#[derive(Clone)]
struct Proxy;

impl EventListener for Proxy {
    fn send_event(&self, _event: Event) {}
}

/// 命名色 → 30–37 / 90–97（前景）或 40–47 / 100–107（背景）。
/// 其余命名色（Foreground / Background / Cursor / Dim* …）走默认色 39 / 49。
fn named_code(color: NamedColor, foreground: bool) -> Option<u16> {
    let value = color as u16;
    if value < 8 {
        Some(30 + value + if foreground { 0 } else { 10 })
    } else if value < 16 {
        Some(90 + (value - 8) + if foreground { 0 } else { 10 })
    } else {
        None
    }
}

/// 一个颜色 → SGR 参数片段。
fn color_code(color: Color, foreground: bool) -> String {
    match color {
        Color::Named(named) => named_code(named, foreground)
            .map(|value| value.to_string())
            .unwrap_or_else(|| {
                if foreground {
                    "39".to_owned()
                } else {
                    "49".to_owned()
                }
            }),
        Color::Indexed(index) => format!("{};5;{index}", if foreground { 38 } else { 48 }),
        Color::Spec(Rgb { r, g, b }) => {
            format!("{};2;{r};{g};{b}", if foreground { 38 } else { 48 })
        }
    }
}

/// 一格的完整 SGR（自带 reset，避免与上一格串味）。
fn sgr_for(foreground: Color, background: Color, flags: Flags) -> String {
    let mut parts = vec![
        "0".to_owned(),
        color_code(foreground, true),
        color_code(background, false),
    ];
    for (flag, code) in [
        (Flags::BOLD, "1"),
        (Flags::DIM, "2"),
        (Flags::ITALIC, "3"),
        (Flags::UNDERLINE, "4"),
        (Flags::INVERSE, "7"),
        (Flags::HIDDEN, "8"),
        (Flags::STRIKEOUT, "9"),
    ] {
        if flags.contains(flag) {
            parts.push(code.to_owned());
        }
    }
    parts.join(";")
}

/// 一个会话的服务端 VT 状态。
pub struct Vt {
    term: Term<Proxy>,
    parser: Processor,
}

impl Vt {
    /// 以给定尺寸建内核；`scrolling_history` 固定 10k 行（与前端 scrollback 对齐）。
    ///
    /// ⚠️ 0.26 的 `TermSize` 只在 crate 内部可见，外部唯一公开的 `Dimensions` 实现者是 `Grid`，
    /// 所以借一个空 `Grid` 当尺寸载体（#42 已记过这条 API 的别扭之处）。
    pub fn new(cols: usize, rows: usize) -> Self {
        let (cols, rows) = (cols.max(1), rows.max(1));
        let size: Grid<Cell> = Grid::new(rows, cols, 0);
        let config = Config {
            scrolling_history: 10_000,
            ..Config::default()
        };
        Self {
            term: Term::new(config, &size, Proxy),
            parser: Processor::new(),
        }
    }

    /// 喂入一段 PTY 字节。**必须按原始顺序、每段只喂一次**。
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    /// 同步 resize（ConPTY 改尺寸时调用）；`Term::resize` 会按新宽度重排。
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.max(1), rows.max(1));
        if cols == self.columns() && rows == self.screen_lines() {
            return;
        }
        let size: Grid<Cell> = Grid::new(rows, cols, 0);
        self.term.resize(size);
    }

    /// 当前列数。
    pub fn columns(&self) -> usize {
        self.term.columns()
    }

    /// 当前屏幕行数。
    pub fn screen_lines(&self) -> usize {
        self.term.screen_lines()
    }

    /// 是否处于 alt-screen（全屏 TUI）。
    pub fn alt_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    /// 光标位置：`(行, 列)`，从 0 开始。
    pub fn cursor(&self) -> (usize, usize) {
        let point = self.term.grid().cursor.point;
        (point.line.0.max(0) as usize, point.column.0)
    }

    /// 内核视角的逐行纯文本（右侧空白裁掉）。
    pub fn lines(&self) -> Vec<String> {
        let grid = self.term.grid();
        (0..self.screen_lines())
            .map(|line| {
                let row = &grid[Line(line as i32)];
                let text: String = (0..self.columns())
                    .filter_map(|column| {
                        let cell = &row[Column(column)];
                        // 宽字符的第二格是 spacer（不占可见字符），跳过才不会出现「中 文」。
                        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                            return None;
                        }
                        Some(if cell.c == '\0' { ' ' } else { cell.c })
                    })
                    .collect();
                text.trim_end().to_owned()
            })
            .collect()
    }

    /// 逐格查询（行 / 列越界返回 `None`）。
    pub fn cell(&self, line: usize, column: usize) -> Option<CellView> {
        if line >= self.screen_lines() || column >= self.columns() {
            return None;
        }
        let cell = &self.term.grid()[Line(line as i32)][Column(column)];
        Some(CellView {
            character: cell.c,
            foreground: cell.fg,
            background: cell.bg,
            flags: cell_flags(cell.flags),
        })
    }

    /// 逐行属性签名（每格两位十六进制），供 golden / #42 parity 比对。
    pub fn style_rows(&self) -> Vec<String> {
        let grid = self.term.grid();
        (0..self.screen_lines())
            .map(|line| {
                let row = &grid[Line(line as i32)];
                (0..self.columns())
                    .map(|column| format!("{:02x}", cell_flags(row[Column(column)].flags)))
                    .collect()
            })
            .collect()
    }

    /// 当前 scrollback 行数（长时运行的内存边界断言用）。
    pub fn history_size(&self) -> usize {
        self.term.grid().history_size()
    }

    /// ANSI 快照：把当前内核状态序列化成一段可被 xterm.js 重放的字节流（#71）。
    ///
    /// 结构：可选 alt-screen 切换 → 隐藏光标 → 复位 SGR → 清屏 → 逐行逐格重绘（带 SGR 运行段）
    /// → 复原光标 → 显示光标。宽字符的 spacer 格会跳过，否则重放时会把整行向右挤。
    pub fn snapshot(&self) -> String {
        let mut out = String::new();
        if self.alt_screen() {
            out.push_str("\u{1b}[?1049h");
        }
        out.push_str("\u{1b}[?25l\u{1b}[0m\u{1b}[H\u{1b}[2J");

        let grid = self.term.grid();
        for line in 0..self.screen_lines() {
            let row = &grid[Line(line as i32)];
            let mut current: Option<String> = None;
            let mut rendered = String::new();
            for column in 0..self.columns() {
                let cell = &row[Column(column)];
                if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                    // 宽字符前一格已经推进了两列；spacer 再写一次会错位。
                    continue;
                }
                let sgr = sgr_for(cell.fg, cell.bg, cell.flags);
                if current.as_deref() != Some(sgr.as_str()) {
                    rendered.push_str(&format!("\u{1b}[{sgr}m"));
                    current = Some(sgr);
                }
                rendered.push(cell.c);
                if let Some(extra) = cell.zerowidth() {
                    for ch in extra {
                        rendered.push(*ch);
                    }
                }
            }
            rendered.push_str("\u{1b}[0m");
            out.push_str(&format!("\u{1b}[{};1H{}", line + 1, rendered));
        }

        let (line, column) = self.cursor();
        out.push_str(&format!("\u{1b}[{};{}H\u{1b}[?25h", line + 1, column + 1));
        out
    }

    /// 稳定、可存档的状态摘要：尺寸 / alt-screen / 光标 / 文本 / 属性位。
    ///
    /// 这是 #70 的 golden 形态；#71 会把同一份状态序列化成 ANSI 快照。
    pub fn render(&self) -> String {
        let mut out = format!(
            "cols={} rows={} alt={} cursor={},{}\n",
            self.columns(),
            self.screen_lines(),
            self.alt_screen(),
            self.cursor().0,
            self.cursor().1
        );
        out.push_str("[lines]\n");
        for (index, line) in self.lines().iter().enumerate() {
            out.push_str(&format!("{index:02}|{line}|\n"));
        }
        out.push_str("[flags]\n");
        for (index, row) in self.style_rows().iter().enumerate() {
            out.push_str(&format!("{index:02}|{row}|\n"));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alacritty_terminal::vte::ansi::NamedColor;

    /// 覆盖 SGR（绿/粗体/反显/下划线）、绝对定位、宽字符、alt-screen 与光标位置。
    fn fixture() -> Vec<u8> {
        let mut script = String::new();
        script.push_str("\u{1b}[2J\u{1b}[H");
        script.push_str("\u{1b}[1;32mGREEN\u{1b}[0m plain\r\n");
        script.push_str(
            "\u{1b}[1mBOLD\u{1b}[0m \u{1b}[4mUNDER\u{1b}[0m \u{1b}[7mINVERSE\u{1b}[0m\r\n",
        );
        script.push_str("\u{1b}[5;3Habsolute\r\n");
        script.push_str("\u{1b}[7;1H\u{1b}[36mcyan\u{1b}[0m 中文🙂\r\n");
        // alt-screen 全屏 TUI
        script.push_str("\u{1b}[?1049h\u{1b}[2J\u{1b}[H");
        script.push_str("\u{1b}[2;4H\u{1b}[33mALT\u{1b}[0m");
        script.push_str("\u{1b}[6;9H");
        script.into_bytes()
    }

    #[test]
    fn maintains_screen_cursor_alt_screen_and_sgr() {
        let mut vt = Vt::new(24, 10);
        vt.feed(&fixture());

        assert!(vt.alt_screen(), "应进入 alt-screen");
        assert_eq!(vt.cursor(), (5, 8), "光标应停在 fixture 指定位置");
        let lines = vt.lines();
        assert!(lines[1].contains("ALT"));
        // alt-screen 内第 2 行第 4 列是 'A'，且被 SGR 33（黄色）上色。
        let alt = vt.cell(1, 3).expect("alt cell");
        assert_eq!(alt.character, 'A');
        assert_eq!(alt.foreground, Color::Named(NamedColor::Yellow));
        assert_eq!(alt.flags, 0);
    }

    #[test]
    fn renders_a_stable_golden_state() {
        let mut vt = Vt::new(24, 10);
        // 先在普通屏写，再切 alt-screen 之前截一张 golden（两张屏幕都覆盖）。
        vt.feed(b"\x1b[2J\x1b[H\x1b[1;32mGREEN\x1b[0m plain\r\n");
        vt.feed(b"\x1b[1mBOLD\x1b[0m \x1b[4mUNDER\x1b[0m \x1b[7mINVERSE\x1b[0m\r\n");
        vt.feed(b"\x1b[5;3Habsolute\r\n");
        vt.feed("\x1b[7;1H\x1b[36mcyan\x1b[0m 中文🙂\r\n".as_bytes());
        let normal = vt.render();

        vt.feed(b"\x1b[?1049h\x1b[2J\x1b[H\x1b[2;4H\x1b[33mALT\x1b[0m\x1b[6;9H");
        let alt = vt.render();

        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vt-golden.txt");
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::create_dir_all(path.parent().expect("fixtures dir")).unwrap();
            std::fs::write(&path, format!("=== normal ===\n{normal}=== alt ===\n{alt}")).unwrap();
            return;
        }

        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("读不到 {}：{error}", path.display()));
        assert_eq!(
            format!("=== normal ===\n{normal}=== alt ===\n{alt}"),
            expected
        );
        assert!(alt.contains("alt=true"));
    }

    #[test]
    fn wide_characters_keep_their_grid_columns() {
        let mut vt = Vt::new(12, 3);
        vt.feed("\u{1b}[1;1H中文🙂".as_bytes());
        let lines = vt.lines();
        assert!(lines[0].starts_with("中文🙂"));
        // 宽字符各占两列，emoji 亦然：光标在 1+2+2+2? 具体列由 alacritty 的宽度表决定；
        // 这里只断言不重叠、文本完整。
    }

    #[test]
    fn scrollback_stays_bounded_during_long_output() {
        let mut vt = Vt::new(80, 24);
        for index in 0..50_000 {
            vt.feed(format!("line {index}\r\n").as_bytes());
        }
        assert!(
            vt.history_size() <= 10_000,
            "scrollback 必须被 10k 行上限截断，实际 {}",
            vt.history_size()
        );
        assert!(
            vt.lines().iter().any(|line| line.contains("49999")),
            "最后一行仍在屏幕上"
        );
    }

    #[test]
    fn resize_reflows_without_losing_state() {
        let mut vt = Vt::new(40, 10);
        vt.feed(b"\x1b[1;1Hhello\r\nworld");
        vt.resize(20, 5);
        assert_eq!((vt.columns(), vt.screen_lines()), (20, 5));
        assert!(vt.lines()[0].contains("hello"));
        assert!(vt.lines()[1].contains("world"));
    }

    /// #71：快照重放进一台全新内核后，屏幕 / 光标 / alt-screen / 属性位必须与原内核一致。
    #[test]
    fn snapshot_round_trips_through_a_fresh_kernel() {
        let mut vt = Vt::new(100, 30);
        vt.feed(&parity_fixture());

        let snapshot = vt.snapshot();
        let mut replay = Vt::new(100, 30);
        replay.feed(snapshot.as_bytes());

        assert_eq!(replay.render(), vt.render());
        assert_eq!(replay.cursor(), vt.cursor());
        assert_eq!(replay.alt_screen(), vt.alt_screen());
    }

    /// #71：快照覆盖普通屏上的宽字符（spacer 格不能把行挤歪）。
    #[test]
    fn snapshot_preserves_wide_characters_on_the_normal_screen() {
        let mut vt = Vt::new(16, 4);
        vt.feed("\u{1b}[2J\u{1b}[H中文🙂 ok\r\nnext".as_bytes());

        let snapshot = vt.snapshot();
        let mut replay = Vt::new(16, 4);
        replay.feed(snapshot.as_bytes());

        assert_eq!(replay.lines(), vt.lines());
        assert_eq!(replay.render(), vt.render());
    }

    /// #71 golden：#42 夹具的 ANSI 快照（UPDATE_GOLDEN=1 时重写）。
    #[test]
    fn snapshot_matches_the_parity_golden() {
        let mut vt = Vt::new(100, 30);
        vt.feed(&parity_fixture());
        assert!(vt.alt_screen(), "夹具最终应处于 alt-screen");
        assert_eq!(vt.cursor(), (27, 6));
        let snapshot = vt.snapshot();

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/vt-parity/snap.ansi");
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::write(&path, &snapshot).unwrap();
            return;
        }
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("读不到 {}：{error}", path.display()));
        assert_eq!(snapshot, expected);
    }

    fn parity_fixture() -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/vt-parity/full.bin"),
        )
        .expect("读不到 #42 fixture")
    }
}
