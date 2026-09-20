//! #42 VT parity 探针：**daemon 侧 Rust VT 内核 → ANSI 快照 → xterm.js 重放一致性**。
//!
//! 产出（写到 `%TEMP%/agm-vt-parity/`，或 `--out` 指定）：
//! - `fixture.bin` 本探针喂给子进程的 VT 脚本（确定、可复现）
//! - `full.bin`    完整原始字节流（= 从头重放的参考）
//! - `tail.bin`    末尾 N KB（“哑管道”基线：有界缓冲只能回放尾巴）
//! - `snap.ansi`   内核算出的 ANSI 快照（C 路线的交付物）
//! - `meta.json`   尺寸 / alt-screen / 光标 / **内核逐格文本**（跨引擎比对的基准）
//!
//! 判定所需的三份材料都在这四个文件里，前端只需重放并逐格比对。

use alacritty_terminal::event::{Event, EventListener};
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::grid::Grid;
use alacritty_terminal::term::cell::Cell;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{Color, NamedColor, Processor, Rgb};

use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Clone)]
struct Proxy;
impl EventListener for Proxy {
    fn send_event(&self, _e: Event) {}
}

pub struct Vt {
    term: Term<Proxy>,
    parser: Processor,
    cols: usize,
    rows: usize,
}

impl Vt {
    pub fn new(cols: usize, rows: usize) -> Self {
        // ⚠️ 0.26 把 `TermSize` 关进了 `#[cfg(test)]`，外部**无法**直接给尺寸；
        //    唯一公开的 `Dimensions` 实现者是 `Grid`，所以借它当尺寸载体。
        //    （这条 API 别扭之处本身是 #42 的结论之一。）
        let size: Grid<Cell> = Grid::new(rows, cols, 0);
        let term = Term::new(Config::default(), &size, Proxy);
        Vt {
            term,
            parser: Processor::new(),
            cols,
            rows,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }

    pub fn alt_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    pub fn cursor(&self) -> (usize, usize) {
        let p = self.term.grid().cursor.point;
        (p.line.0.max(0) as usize, p.column.0)
    }

    /// 内核视角的「参考屏幕」：逐行纯文本（右裁剪空白）
    pub fn lines(&self) -> Vec<String> {
        let grid = self.term.grid();
        (0..self.rows)
            .map(|l| {
                let row = &grid[Line(l as i32)];
                let s: String = (0..self.cols).map(|c| row[Column(c)].c).collect();
                s.trim_end().to_string()
            })
            .collect()
    }

    /// 按列对齐的字符矩阵（满宽；宽字符的占位格为空格）——前端按列比对文本用
    pub fn cell_rows(&self) -> Vec<String> {
        let grid = self.term.grid();
        (0..self.rows)
            .map(|l| {
                let row = &grid[Line(l as i32)];
                let mut s = String::with_capacity(self.cols);
                for c in 0..self.cols {
                    let ch = row[Column(c)].c;
                    s.push(if ch == '\0' { ' ' } else { ch });
                }
                s
            })
            .collect()
    }

    /// 逐格**属性签名**（每格两位十六进制），用于跨引擎比对样式是否丢失。
    /// 位定义（前后端必须一致）：1=INVERSE 2=BOLD 4=ITALIC 8=UNDERLINE 16=DIM 32=HIDDEN 64=STRIKEOUT
    pub fn style_rows(&self) -> Vec<String> {
        let grid = self.term.grid();
        (0..self.rows)
            .map(|l| {
                let row = &grid[Line(l as i32)];
                (0..self.cols)
                    .map(|c| format!("{:02x}", flag_bits(row[Column(c)].flags)))
                    .collect()
            })
            .collect()
    }

    /// ANSI 快照：先切 alt-screen（若内核在 alt），清屏后逐行重绘并复原光标。
    pub fn snapshot(&self) -> String {
        let mut out = String::new();
        if self.alt_screen() {
            out.push_str("\u{1b}[?1049h");
        }
        out.push_str("\u{1b}[?25l\u{1b}[0m\u{1b}[H\u{1b}[2J");
        let grid = self.term.grid();
        for l in 0..self.rows {
            let row = &grid[Line(l as i32)];
            let mut line = String::new();
            let mut cur: Option<String> = None;
            for c in 0..self.cols {
                let cell = &row[Column(c)];
                let sgr = sgr_for(cell.fg, cell.bg, cell.flags);
                if cur.as_deref() != Some(sgr.as_str()) {
                    line.push_str(&format!("\u{1b}[{sgr}m"));
                    cur = Some(sgr);
                }
                line.push(cell.c);
            }
            line.push_str("\u{1b}[0m");
            out.push_str(&format!("\u{1b}[{};1H{}", l + 1, line));
        }
        let (cl, cc) = self.cursor();
        out.push_str(&format!("\u{1b}[{};{}H\u{1b}[?25h", cl + 1, cc + 1));
        out
    }
}

/// 按**位置**映射命名色，不列举变体名（对版本差异更稳）。
/// 前 8 个是标准色、接着 8 个是亮色；其余（Foreground/Background/Cursor/Dim* 等）走默认。
/// 位定义与前端 `VtParity.vue` 保持一致
fn flag_bits(f: Flags) -> u8 {
    let mut b = 0u8;
    if f.contains(Flags::INVERSE) {
        b |= 1;
    }
    if f.contains(Flags::BOLD) {
        b |= 2;
    }
    if f.contains(Flags::ITALIC) {
        b |= 4;
    }
    if f.contains(Flags::UNDERLINE) {
        b |= 8;
    }
    if f.contains(Flags::DIM) {
        b |= 16;
    }
    if f.contains(Flags::HIDDEN) {
        b |= 32;
    }
    if f.contains(Flags::STRIKEOUT) {
        b |= 64;
    }
    b
}

fn named_code(n: NamedColor, fg: bool) -> Option<u16> {
    let v = n as u16;
    if v < 8 {
        Some(30 + v + if fg { 0 } else { 10 })
    } else if v < 16 {
        Some(90 + (v - 8) + if fg { 0 } else { 10 })
    } else {
        None
    }
}

fn color_code(c: Color, fg: bool) -> String {
    match c {
        Color::Named(n) => named_code(n, fg)
            .map(|v| v.to_string())
            .unwrap_or_else(|| if fg { "39".into() } else { "49".into() }),
        Color::Indexed(i) => format!("{};5;{}", if fg { 38 } else { 48 }, i),
        Color::Spec(Rgb { r, g, b }) => {
            format!("{};2;{};{};{}", if fg { 38 } else { 48 }, r, g, b)
        }
    }
}

/// 生成一条 SGR（自带 reset 起手，避免与上一格串味）
fn sgr_for(fg: Color, bg: Color, flags: Flags) -> String {
    let mut parts = vec!["0".to_string()];
    parts.push(color_code(fg, true));
    parts.push(color_code(bg, false));
    if flags.contains(Flags::BOLD) {
        parts.push("1".into());
    }
    if flags.contains(Flags::DIM) {
        parts.push("2".into());
    }
    if flags.contains(Flags::ITALIC) {
        parts.push("3".into());
    }
    if flags.contains(Flags::UNDERLINE) {
        parts.push("4".into());
    }
    if flags.contains(Flags::INVERSE) {
        parts.push("7".into());
    }
    if flags.contains(Flags::HIDDEN) {
        parts.push("8".into());
    }
    if flags.contains(Flags::STRIKEOUT) {
        parts.push("9".into());
    }
    parts.join(";")
}

// ───────────────────────── fixture：一段确定、可复现的 VT 脚本 ─────────────────────────

/// 覆盖 ticket 要求的场景：SGR、绝对定位、宽字符/emoji、**alt-screen 全屏**、光标位置。
pub fn fixture_script(cols: usize, rows: usize) -> Vec<u8> {
    let mut s = String::new();
    // 1) 普通屏：颜色 + 定位 + 宽字符 + emoji
    s.push_str("\u{1b}[2J\u{1b}[H");
    s.push_str("\u{1b}[1;32mAGEMINAL\u{1b}[0m parity fixture\r\n");
    s.push_str("\u{1b}[33m\u{1b}[1mwide:\u{1b}[0m 中文宽字符 ok  \u{1b}[36memoji: \u{1b}[0m🙂\r\n");
    s.push_str("\u{1b}[5;10Habsolute\u{1b}[0m pos (row5,col10)\r\n");
    s.push_str("\u{1b}[7;1Hline7-left\r\n");
    // 2) alt-screen 全屏 TUI
    s.push_str("\u{1b}[?1049h");
    s.push_str("\u{1b}[2J\u{1b}[H");
    s.push_str("\u{1b}[7m\u{1b}[1;1H");
    s.push_str(&" ".repeat(cols));
    s.push_str(&format!("\u{1b}[0m\u{1b}[1;1H\u{1b}[1m AGEMINAL TUI (alt-screen) \u{1b}[0m"));
    // 边框
    for r in 2..=rows.saturating_sub(1) {
        s.push_str(&format!("\u{1b}[{r};1H\u{1b}[34m|\u{1b}[0m"));
        s.push_str(&format!("\u{1b}[{r};{}H\u{1b}[34m|\u{1b}[0m", cols));
    }
    s.push_str(&format!("\u{1b}[{};1H\u{1b}[34m{}\u{1b}[0m", rows, "-".repeat(cols)));
    // 内容：带属性的一行
    s.push_str(&format!("\u{1b}[3;3H\u{1b}[32mstatus:\u{1b}[0m \u{1b}[7mRUNNING\u{1b}[0m"));
    s.push_str(&format!("\u{1b}[5;3H\u{1b}[35mworktree\u{1b}[0m main"));
    s.push_str(&format!("\u{1b}[6;3H\u{1b}[36magent\u{1b}[0m opencode \u{1b}[33m#42\u{1b}[0m"));
    s.push_str(&format!("\u{1b}[8;3H\u{1b}[1;31mERROR\u{1b}[0m sample line"));
    s.push_str(&format!("\u{1b}[9;3H\u{1b}[4munderlined\u{1b}[0m \u{1b}[9mstrike\u{1b}[0m"));
    // 收尾：把光标放在一个确定位置（这里**故意留在 alt-screen 内**）
    let (cr, cc) = (rows.saturating_sub(2).max(1), 7usize);
    s.push_str(&format!("\u{1b}[{cr};{cc}H"));
    s.into_bytes()
}

// ───────────────────────── 驱动 ─────────────────────────

#[derive(Serialize)]
struct Meta {
    cols: usize,
    rows: usize,
    alt_screen: bool,
    cursor: [usize; 2],
    full_bytes: usize,
    tail_bytes: usize,
    snapshot_bytes: usize,
    /// 内核视角的逐行文本（跨引擎比对基准；右裁剪空白）
    lines: Vec<String>,
    /// 逐行、逐格属性签名（每格两位 hex；位定义见 `Vt::style_rows`）
    style_rows: Vec<String>,
    /// 逐行**按列对齐**的字符矩阵（满宽、不裁剪；宽字符占位格记为空格）
    cell_rows: Vec<String>,
    fixture_covers: Vec<&'static str>,
}

fn write_all(dir: &Path, cols: usize, rows: usize, stream: &[u8], tail_keep: usize) {
    let _ = std::fs::create_dir_all(dir);
    let mut vt = Vt::new(cols, rows);
    vt.feed(stream);
    let snap = vt.snapshot();
    let tail: &[u8] = if stream.len() > tail_keep {
        &stream[stream.len() - tail_keep..]
    } else {
        stream
    };
    let meta = Meta {
        cols,
        rows,
        alt_screen: vt.alt_screen(),
        cursor: [vt.cursor().0, vt.cursor().1],
        full_bytes: stream.len(),
        tail_bytes: tail.len(),
        snapshot_bytes: snap.len(),
        lines: vt.lines(),
        style_rows: vt.style_rows(),
        cell_rows: vt.cell_rows(),
        fixture_covers: vec![
            "SGR 颜色/粗体/下划线/删除线/反显",
            "绝对定位 ESC[r;cH",
            "宽字符与 emoji",
            "alt-screen 全屏（ESC[?1049h）",
            "边框与逐行属性",
            "确定的收尾光标位置",
        ],
    };
    let _ = std::fs::write(dir.join("full.bin"), stream);
    let _ = std::fs::write(dir.join("tail.bin"), tail);
    let _ = std::fs::write(dir.join("snap.ansi"), snap.as_bytes());
    let _ = std::fs::write(dir.join("meta.json"), serde_json::to_string_pretty(&meta).unwrap());
    println!("已写出 {}", dir.display());
    println!(
        "  full={} tail={} snapshot={} 字节 · alt_screen={} cursor=({},{})",
        meta.full_bytes, meta.tail_bytes, meta.snapshot_bytes, meta.alt_screen, meta.cursor[0], meta.cursor[1]
    );
    println!("  内核逐行文本（前 12 行）：");
    for (i, l) in meta.lines.iter().take(12).enumerate() {
        println!("    {i:>3}| {l}");
    }
}

/// 从文件读字节流（可在 Linux/WSL 上先把整条链验通）
pub fn replay(input: PathBuf, out: PathBuf, cols: usize, rows: usize, tail_keep: usize) -> i32 {
    let stream = match std::fs::read(&input) {
        Ok(v) => v,
        Err(e) => {
            println!("读入失败 {}：{e}", input.display());
            return 1;
        }
    };
    println!("== VT parity（replay）==\n输入 {}（{} 字节）", input.display(), stream.len());
    write_all(&out, cols, rows, &stream, tail_keep);
    0
}

/// 起 ConPTY 跑 fixture，再对同一段流做快照
#[cfg(windows)]
pub fn conpty(out: PathBuf, cols: usize, rows: usize, tail_keep: usize, seconds: u64) -> i32 {
    use crate::conpty::{self, PtySpawn};

    let _ = std::fs::create_dir_all(&out);
    let script = fixture_script(cols, rows);
    let script_path = out.join("fixture.bin");
    if std::fs::write(&script_path, &script).is_err() {
        println!("写 fixture 失败");
        return 1;
    }

    // 子进程：`type <脚本>` 把原始 VT 字节直接吐到 stdout（不做任何解释），
    // 之后用 timeout 挂住，让会话保持在该屏幕状态以便快照。
    // ⚠️ 不加 `/c` 的外层引号：cmd 自己的 `/c` 会取余下整行，再加引号会与路径引号嵌套。
    //    `conpty` 只给第一个 token 加引号，其余原样传给 cmd 解析。
    let cmdline = format!(
        "cmd.exe /c type \"{}\" & timeout /t {} /nobreak >nul",
        script_path.display(),
        seconds + 5
    );
    let req = PtySpawn {
        shell: cmdline,
        cols: cols as u16,
        rows: rows as u16,
    };
    let (pty, mut rd) = match conpty::spawn(&req) {
        Ok(v) => v,
        Err(e) => {
            println!("ConPTY 起进程失败：{e}");
            return 1;
        }
    };
    println!("== VT parity（ConPTY）==\n子进程 pid={} · 观察 {seconds}s", pty.pid());

    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match rd.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut stream: Vec<u8> = Vec::new();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    while std::time::Instant::now() < deadline {
        match rx.recv_timeout(std::time::Duration::from_millis(200)) {
            Ok(d) => stream.extend_from_slice(&d),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
    }
    let _ = pty.close();
    println!("收到 {} 字节", stream.len());
    write_all(&out, cols, rows, &stream, tail_keep);
    0
}
