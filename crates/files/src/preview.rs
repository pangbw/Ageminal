//! 文件只读预览的内容判定与截断（#60；D1–D5 #20；REQUIREMENTS.md §9）。
//!
//! 判定顺序（D1 #20，**不可调换**）：
//!
//! 1. 图片扩展名 → 图片（前端走 `<img>`，D6 #20）；
//! 2. 已知二进制扩展名黑名单 → 二进制；
//! 3. 前缀含 NUL 字节 → 二进制；
//! 4. 严格 UTF-8 解码成功（剥 BOM）→ 文本；
//! 5. 无效 UTF-8 → **lossy 解码 + 警告**（D2 #20：不再判二进制，GBK 延后）。
//!
//! 大文件护栏（D5 #20 / §9）：文本最多回传 [`MAX_TEXT_BYTES`]（2 MiB）、最多 [`MAX_LINES`]
//! （50k）行——**Rust 只回传前 N 字节**，不把整文件读进内存；单行超过 [`MAX_LINE_BYTES`]
//! （100 KiB）时置 `long_line` 供前端提示。图片上限 [`MAX_IMAGE_BYTES`]（20 MiB），超过只回
//! 元数据、不带数据。
//!
//! 换行（D4 #20）：只读不写回；`\n` / `\r\n` / `\r` 都按分隔符处理，**不规范化**——这里只用来
//! 数行与判超长行，返回的文本保持原样。

use std::io::Read as _;
use std::path::Path;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

use crate::error::FilesError;

/// 文本回传上限：2 MiB（D5 #20 / §9）。
pub const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;
/// 文本行数上限：50k（D5 #20 / §9）。
pub const MAX_LINES: usize = 50_000;
/// 单行字节上限：100 KiB（D5 #20 / §9，超出仅提示）。
pub const MAX_LINE_BYTES: usize = 100 * 1024;
/// 图片上限：20 MiB（C4 #12 / §9），超过不加载数据。
pub const MAX_IMAGE_BYTES: u64 = 20 * 1024 * 1024;

/// 内容类型（判定顺序的产物，D1 #20）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewKind {
    Text,
    Image,
    Binary,
}

/// 一次预览读取的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preview {
    Text(TextPreview),
    Image(ImagePreview),
    Binary(BinaryPreview),
}

/// 文本预览。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextPreview {
    /// 回传的文本（剥 BOM、必要时截断；`\n` / `\r\n` / `\r` 原样保留）。
    pub text: String,
    /// 无效 UTF-8，已 lossy 解码（D2 #20，前端顶部警示）。
    pub lossy: bool,
    /// 原文带 UTF-8 BOM（已剥掉）。
    pub bom: bool,
    /// 命中了字节上限或行数上限。
    pub truncated: bool,
    /// 回传文本的行数（`\n` / `\r\n` / `\r` 都算分隔符）。
    pub lines: usize,
    /// 含超过 [`MAX_LINE_BYTES`] 的单行（D5 #20，前端顶部提示）。
    pub long_line: bool,
    /// 文件总字节数。
    pub total_bytes: u64,
}

/// 图片预览。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImagePreview {
    /// `data:` URL；文件超过 [`MAX_IMAGE_BYTES`] 时为 `None`。
    pub data_url: Option<String>,
    /// 超过图片上限，未加载数据。
    pub too_large: bool,
    /// 文件总字节数。
    pub total_bytes: u64,
}

/// 二进制预览（只回元数据，不读内容）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryPreview {
    /// 文件总字节数。
    pub total_bytes: u64,
}

/// 预览的护栏参数（生产用 [`PreviewLimits::default`]；测试注入小值）。
#[derive(Debug, Clone, Copy)]
pub struct PreviewLimits {
    pub text_bytes: usize,
    pub lines: usize,
    pub line_bytes: usize,
    pub image_bytes: u64,
}

impl Default for PreviewLimits {
    fn default() -> Self {
        Self {
            text_bytes: MAX_TEXT_BYTES,
            lines: MAX_LINES,
            line_bytes: MAX_LINE_BYTES,
            image_bytes: MAX_IMAGE_BYTES,
        }
    }
}

/// 读取 `path`（须在 worktree 根 `root` 之内）的预览。
///
/// 判定顺序与护栏见模块文档；图片超过上限只回元数据。
pub fn read_preview(root: &Path, path: &Path) -> Result<Preview, FilesError> {
    read_preview_with(root, path, PreviewLimits::default())
}

fn read_preview_with(
    root: &Path,
    path: &Path,
    limits: PreviewLimits,
) -> Result<Preview, FilesError> {
    crate::relative_to(root, path)?;

    let metadata = std::fs::metadata(path).map_err(|source| io_error(path, source))?;
    if metadata.is_dir() {
        return Err(FilesError::NotAFile {
            path: path.to_path_buf(),
        });
    }
    let total_bytes = metadata.len();
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    if let Some(media_type) = image_media_type(&extension) {
        return read_image(path, media_type, total_bytes, limits);
    }
    if is_binary_extension(&extension) {
        return Ok(Preview::Binary(BinaryPreview { total_bytes }));
    }

    // 只读前缀：多读 1 字节以判定「后面还有内容」。
    let file = std::fs::File::open(path).map_err(|source| io_error(path, source))?;
    let mut buffer = Vec::new();
    let read_cap = limits.text_bytes.saturating_add(1) as u64;
    file.take(read_cap)
        .read_to_end(&mut buffer)
        .map_err(|source| io_error(path, source))?;
    let byte_truncated = buffer.len() > limits.text_bytes;
    buffer.truncate(limits.text_bytes);

    if buffer.contains(&0) {
        return Ok(Preview::Binary(BinaryPreview { total_bytes }));
    }

    let (decoded, bom, lossy) = decode_text(buffer, byte_truncated);
    let bounded = bound_lines(&decoded, limits.lines, limits.line_bytes);
    Ok(Preview::Text(TextPreview {
        text: bounded.text,
        lossy,
        bom,
        truncated: byte_truncated || bounded.truncated,
        lines: bounded.lines,
        long_line: bounded.long_line,
        total_bytes,
    }))
}

fn read_image(
    path: &Path,
    media_type: &str,
    total_bytes: u64,
    limits: PreviewLimits,
) -> Result<Preview, FilesError> {
    if total_bytes > limits.image_bytes {
        return Ok(Preview::Image(ImagePreview {
            data_url: None,
            too_large: true,
            total_bytes,
        }));
    }
    let bytes = std::fs::read(path).map_err(|source| io_error(path, source))?;
    let data_url = format!("data:{media_type};base64,{}", STANDARD.encode(&bytes));
    Ok(Preview::Image(ImagePreview {
        data_url: Some(data_url),
        too_large: false,
        total_bytes,
    }))
}

fn io_error(path: &Path, source: std::io::Error) -> FilesError {
    if source.kind() == std::io::ErrorKind::NotFound {
        FilesError::NotFound {
            path: path.to_path_buf(),
        }
    } else {
        FilesError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// 按判定顺序第 4 / 5 步解码文本（剥 BOM，无效 UTF-8 走 lossy）。
///
/// `truncated` 表示 `bytes` 是从更大的文件里切出来的前缀：此时一个**未完成**的多字节字符
/// 序列是正常的（切在字符中间），丢掉它即可，不算「无效 UTF-8」。
fn decode_text(mut bytes: Vec<u8>, truncated: bool) -> (String, bool, bool) {
    let bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
    if bom {
        bytes.drain(..3);
    }
    match std::str::from_utf8(&bytes) {
        Ok(text) => (text.to_owned(), bom, false),
        Err(error) if truncated && error.error_len().is_none() => {
            let prefix = &bytes[..error.valid_up_to()];
            (String::from_utf8_lossy(prefix).into_owned(), bom, false)
        }
        Err(_) => (String::from_utf8_lossy(&bytes).into_owned(), bom, true),
    }
}

/// 按行数上限截断，并统计行数与超长行。
struct BoundedLines {
    text: String,
    lines: usize,
    long_line: bool,
    truncated: bool,
}

fn bound_lines(text: &str, max_lines: usize, max_line_bytes: usize) -> BoundedLines {
    let bytes = text.as_bytes();
    let mut lines = 1usize;
    let mut line_start = 0usize;
    let mut long_line = false;
    let mut cut: Option<usize> = None;
    let mut index = 0usize;

    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'\n' || byte == b'\r' {
            if index - line_start > max_line_bytes {
                long_line = true;
            }
            let step = if byte == b'\r' && index + 1 < bytes.len() && bytes[index + 1] == b'\n' {
                2
            } else {
                1
            };
            lines += 1;
            index += step;
            line_start = index;
            if lines == max_lines + 1 && cut.is_none() {
                cut = Some(line_start);
            }
        } else {
            index += 1;
        }
    }
    if bytes.len() - line_start > max_line_bytes {
        long_line = true;
    }

    match cut {
        Some(offset) if offset < bytes.len() => BoundedLines {
            text: text[..offset].to_owned(),
            lines: max_lines,
            long_line,
            truncated: true,
        },
        _ => BoundedLines {
            text: text.to_owned(),
            lines,
            long_line,
            truncated: false,
        },
    }
}

/// 图片扩展名 → MIME；不是图片返回 `None`。`.svg` **不算图片**，当文本看（D3 #20）。
fn image_media_type(extension: &str) -> Option<&'static str> {
    Some(match extension {
        "png" => "image/png",
        "jpg" | "jpeg" | "jpe" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        "avif" => "image/avif",
        "tif" | "tiff" => "image/tiff",
        _ => return None,
    })
}

/// 已知二进制扩展名黑名单（判定顺序第 2 步）。带点的小写扩展名比较。
fn is_binary_extension(extension: &str) -> bool {
    matches!(
        extension,
        // 可执行 / 目标文件 / 字节码
        "exe" | "dll" | "so" | "dylib" | "a" | "lib" | "o" | "obj" | "class" | "jar"
            | "war" | "pyc" | "pyd" | "pyo" | "wasm"
        // 压缩 / 归档
            | "zip" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "7z" | "rar" | "tar" | "whl" | "lz4"
        // 文档 / 出版
            | "pdf" | "ps" | "eps" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx"
        // 字体
            | "woff" | "woff2" | "ttf" | "otf" | "eot"
        // 数据库 / 裸数据
            | "bin" | "dat" | "db" | "sqlite" | "sqlite3" | "mdb"
        // 音频
            | "mp3" | "wav" | "flac" | "ogg" | "oga" | "m4a" | "aac" | "opus" | "wma"
        // 视频
            | "mp4" | "m4v" | "mov" | "avi" | "mkv" | "webm" | "wmv" | "flv"
        // 磁盘镜像
            | "iso" | "img" | "dmg"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用的窄护栏，避免真造 2 MiB / 50k 行的样本。
    fn limits() -> PreviewLimits {
        PreviewLimits {
            text_bytes: 64,
            lines: 4,
            line_bytes: 8,
            image_bytes: 32,
        }
    }

    fn workspace(name: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("ageminal-preview-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn write(root: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = root.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn reads_plain_utf8_text_and_counts_lines() {
        let root = workspace("text");
        let path = write(&root, "a.txt", b"hello\nworld\n");
        let preview = read_preview_with(&root, &path, limits()).unwrap();

        match preview {
            Preview::Text(text) => {
                assert_eq!(text.text, "hello\nworld\n");
                assert_eq!(text.lines, 3); // 结尾换行 = 空的第 3 行
                assert!(!text.lossy);
                assert!(!text.bom);
                assert!(!text.truncated);
                assert_eq!(text.total_bytes, 12);
            }
            other => panic!("expected text, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn splits_on_crlf_and_cr_without_normalizing() {
        let root = workspace("lines");
        let path = write(&root, "a.txt", b"a\r\nb\rc\n");
        let preview = read_preview_with(&root, &path, limits()).unwrap();

        match preview {
            Preview::Text(text) => {
                assert_eq!(text.text, "a\r\nb\rc\n"); // 原样保留，不规范化
                assert_eq!(text.lines, 4);
            }
            other => panic!("expected text, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn strips_a_utf8_bom() {
        let root = workspace("bom");
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("hi".as_bytes());
        let path = write(&root, "a.txt", &bytes);
        let preview = read_preview_with(&root, &path, limits()).unwrap();

        match preview {
            Preview::Text(text) => {
                assert_eq!(text.text, "hi");
                assert!(text.bom);
                assert!(!text.lossy);
            }
            other => panic!("expected text, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn decodes_invalid_utf8_lossily_with_a_warning() {
        let root = workspace("gbk");
        // 「你好」的 GBK 编码：不是有效 UTF-8。
        let path = write(&root, "a.txt", &[0xC4, 0xE3, 0xBA, 0xC3]);
        let preview = read_preview_with(&root, &path, limits()).unwrap();

        match preview {
            Preview::Text(text) => {
                assert!(text.lossy);
                assert!(text.text.contains('\u{FFFD}'));
            }
            other => panic!("expected text, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn treats_nul_bytes_as_binary() {
        let root = workspace("nul");
        let path = write(&root, "a.bin.txt", b"abc\0def");
        let preview = read_preview_with(&root, &path, limits()).unwrap();
        assert!(matches!(preview, Preview::Binary(_)));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn treats_blacklisted_extensions_as_binary_without_reading() {
        let root = workspace("blacklist");
        // 内容其实是合法 UTF-8，但扩展名在黑名单里。
        let path = write(&root, "archive.zip", b"not really a zip");
        let preview = read_preview_with(&root, &path, limits()).unwrap();
        match preview {
            Preview::Binary(binary) => assert_eq!(binary.total_bytes, 16),
            other => panic!("expected binary, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn renders_images_as_data_urls() {
        let root = workspace("image");
        let path = write(&root, "a.png", &[1, 2, 3, 4]);
        let preview = read_preview_with(&root, &path, limits()).unwrap();

        match preview {
            Preview::Image(image) => {
                assert!(!image.too_large);
                let url = image.data_url.expect("data url");
                assert!(url.starts_with("data:image/png;base64,"));
                assert_eq!(image.total_bytes, 4);
            }
            other => panic!("expected image, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn skips_reading_oversized_images() {
        let root = workspace("image-big");
        let path = write(&root, "a.jpg", &[0u8; 64]);
        let preview = read_preview_with(&root, &path, limits()).unwrap();

        match preview {
            Preview::Image(image) => {
                assert!(image.too_large);
                assert!(image.data_url.is_none());
                assert_eq!(image.total_bytes, 64);
            }
            other => panic!("expected image, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn truncates_text_at_the_byte_cap() {
        let root = workspace("bytes");
        let path = write(&root, "a.txt", b"0123456789");
        // 护栏 5 字节。
        let preview = read_preview_with(
            &root,
            &path,
            PreviewLimits {
                text_bytes: 5,
                ..limits()
            },
        )
        .unwrap();

        match preview {
            Preview::Text(text) => {
                assert_eq!(text.text, "01234");
                assert!(text.truncated);
                assert_eq!(text.total_bytes, 10);
            }
            other => panic!("expected text, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn tolerates_a_multibyte_char_cut_by_the_byte_cap() {
        let root = workspace("cut-char");
        // 「好」= E5 A5 BD；护栏切在字符中间时不该被当成无效 UTF-8。
        let path = write(&root, "a.txt", "ab好".as_bytes());
        let preview = read_preview_with(
            &root,
            &path,
            PreviewLimits {
                text_bytes: 3,
                ..limits()
            },
        )
        .unwrap();

        match preview {
            Preview::Text(text) => {
                assert_eq!(text.text, "ab");
                assert!(!text.lossy);
                assert!(text.truncated);
            }
            other => panic!("expected text, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn truncates_at_the_line_cap() {
        let root = workspace("lines-cap");
        let path = write(&root, "a.txt", b"1\n2\n3\n4\n5\n6");
        let preview = read_preview_with(
            &root,
            &path,
            PreviewLimits {
                lines: 2,
                ..limits()
            },
        )
        .unwrap();

        match preview {
            Preview::Text(text) => {
                assert_eq!(text.text, "1\n2\n");
                assert_eq!(text.lines, 2);
                assert!(text.truncated);
            }
            other => panic!("expected text, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn keeps_a_trailing_newline_at_the_line_cap_untruncated() {
        let root = workspace("lines-exact");
        let path = write(&root, "a.txt", b"1\n2\n");
        let preview = read_preview_with(
            &root,
            &path,
            PreviewLimits {
                lines: 2,
                ..limits()
            },
        )
        .unwrap();

        match preview {
            Preview::Text(text) => {
                assert!(!text.truncated); // 第 3 行是空的，没有内容被丢
                assert_eq!(text.text, "1\n2\n");
                assert_eq!(text.lines, 3);
            }
            other => panic!("expected text, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn flags_long_lines() {
        let root = workspace("long-line");
        let path = write(&root, "a.txt", b"0123456789\nok");
        let preview = read_preview_with(&root, &path, limits()).unwrap();

        match preview {
            Preview::Text(text) => assert!(text.long_line),
            other => panic!("expected text, got {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn treats_svg_as_text() {
        let root = workspace("svg");
        let path = write(&root, "a.svg", b"<svg></svg>");
        let preview = read_preview_with(&root, &path, limits()).unwrap();
        assert!(matches!(preview, Preview::Text(_)));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_paths_outside_the_root() {
        let root = workspace("outside");
        let outside = root.parent().unwrap().join("ageminal-preview-outside.txt");
        std::fs::write(&outside, b"nope").unwrap();
        let error = read_preview_with(&root, &outside, limits()).unwrap_err();
        assert!(matches!(error, FilesError::OutsideRoot { .. }));
        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reports_a_missing_file() {
        let root = workspace("missing");
        let error = read_preview_with(&root, &root.join("nope.txt"), limits()).unwrap_err();
        assert!(matches!(error, FilesError::NotFound { .. }));
        let _ = std::fs::remove_dir_all(&root);
    }
}
