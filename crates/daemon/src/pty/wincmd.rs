//! Windows 的两个「约定俗成」：命令行怎么加引号、环境块怎么拼。
//!
//! 与 ConPTY 生命周期无关，可以单独读、单独测——所以从 `conpty.rs` 里拆出来。

use windows_sys::Win32::System::Environment::{FreeEnvironmentStringsW, GetEnvironmentStringsW};

use super::{windows_error, PtyError};

/// 拼命令行（`CreateProcessW` 的 `lpCommandLine`）。
pub(super) fn command_line(program: &str, args: &[String]) -> String {
    let mut line = String::new();
    append_argument(&mut line, program);
    for arg in args {
        line.push(' ');
        append_argument(&mut line, arg);
    }
    line
}

/// 按 `CommandLineToArgvW` 的反解析规则追加一个参数。
///
/// 三件事：需要时整体加引号；引号内的 `"` 用 `\"` 转义、紧邻引号的反斜杠要翻倍；结尾的反斜杠
/// 也要翻倍，否则会把**收尾**引号转义掉（`"C:\a b\"` 会被解析成未闭合）。
fn append_argument(line: &mut String, argument: &str) {
    let needs_quotes = argument.is_empty() || argument.contains([' ', '\t', '"']);

    if !needs_quotes {
        line.push_str(argument);
        return;
    }

    line.push('"');
    let mut backslashes = 0usize;
    for character in argument.chars() {
        match character {
            '\\' => backslashes += 1,
            '"' => {
                // 2n 个反斜杠 + 引号 → n 个反斜杠 + 转义引号。
                line.extend(std::iter::repeat('\\').take(backslashes * 2 + 1));
                line.push('"');
                backslashes = 0;
            }
            other => {
                line.extend(std::iter::repeat('\\').take(backslashes));
                line.push(other);
                backslashes = 0;
            }
        }
    }
    // 结尾反斜杠若紧贴收尾引号，必须翻倍。
    line.extend(std::iter::repeat('\\').take(backslashes * 2));
    line.push('"');
}

/// 构造 `CREATE_UNICODE_ENVIRONMENT` 要的环境块：继承 daemon 的环境 + `overrides`。
///
/// Windows 的三条要求都在这：**按名字排序**、**双 NUL 结尾**、名字**不区分大小写**。
pub(super) fn environment_block(overrides: &[(String, String)]) -> Result<Vec<u16>, PtyError> {
    Ok(merge_environment(inherited_environment()?, overrides))
}

/// 纯逻辑部分：合并 + 排序 + 编码。跟系统调用分开，才好测「空环境」这类边界。
fn merge_environment(
    mut entries: Vec<(String, String)>,
    overrides: &[(String, String)],
) -> Vec<u16> {
    for (name, value) in overrides {
        // Windows 的环境变量名不区分大小写：覆盖时要连不同大小写的同名项一起删掉。
        let upper = name.to_uppercase();
        entries.retain(|(existing, _)| existing.to_uppercase() != upper);
        entries.push((name.clone(), value.clone()));
    }

    // 排序要的是 Windows 的「序数、不区分大小写」——比较**大写的 UTF-16 单元**最接近它
    // （直接比字符串的话，非 BMP 字符的序关系会和 Windows 不一致）。
    entries
        .sort_by_cached_key(|(name, _)| name.to_uppercase().encode_utf16().collect::<Vec<u16>>());

    let mut block = Vec::new();
    for (name, value) in entries {
        block.extend(name.encode_utf16());
        block.push(u16::from(b'='));
        block.extend(value.encode_utf16());
        block.push(0);
    }
    // 双 NUL 结尾。有项时最后一项已经带了一个 NUL，补一个即可；**空块要补两个**，
    // 否则 Windows 会把块后面的内存当成环境继续读。
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    block
}

/// 读 daemon 自己的环境，拆成 `(名字, 值)`。
fn inherited_environment() -> Result<Vec<(String, String)>, PtyError> {
    // SAFETY：返回的是一块由系统分配、直到 `FreeEnvironmentStringsW` 为止都有效的
    // `UTF-16` 复 NUL 串；我们只读，且在下面原样还回去。
    let block = unsafe { GetEnvironmentStringsW() };
    if block.is_null() {
        return Err(windows_error("GetEnvironmentStringsW"));
    }

    let mut entries = Vec::new();
    let mut cursor = block;
    loop {
        // 这一行的长度（到下一个 NUL 为止）。
        let length = unsafe { (0..).take_while(|offset| *cursor.add(*offset) != 0).count() };
        if length == 0 {
            break;
        }
        let line = unsafe { std::slice::from_raw_parts(cursor, length) };
        if let Some((name, value)) = String::from_utf16_lossy(line).split_once('=') {
            // `=C:=C:\...` 这类伪变量名字是空串，跳过（MS 文档要求枚举时忽略它们）。
            if !name.is_empty() {
                entries.push((name.to_owned(), value.to_owned()));
            }
        }
        cursor = unsafe { cursor.add(length + 1) };
    }
    // SAFETY：`block` 就是上面 `GetEnvironmentStringsW` 给的那块，没被移动、没被改。
    unsafe { FreeEnvironmentStringsW(block) };

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    #[test]
    fn command_line_quotes_like_command_line_to_argv_w() {
        assert_eq!(command_line("cmd.exe", &[]), "cmd.exe");
        assert_eq!(
            command_line(
                r"C:\Program Files\Git\bin\bash.exe",
                &args(&["--login", "-i"])
            ),
            r#""C:\Program Files\Git\bin\bash.exe" --login -i"#
        );
        // 空格 → 加引号。
        assert_eq!(command_line("x", &args(&["a b"])), r#"x "a b""#);
        // 空参数 → 一个空引号对。
        assert_eq!(command_line("x", &args(&[""])), r#"x """#);
        // 引号本身 → `\"`。
        assert_eq!(command_line("x", &args(&[r#"a"b"#])), r#"x "a\"b""#);
        // 结尾反斜杠必须翻倍，否则会转义掉收尾引号。
        assert_eq!(
            command_line("x", &args(&[r"C:\dir with space\"])),
            r#"x "C:\dir with space\\""#
        );
        // 制表符也算需要引号。
        assert_eq!(command_line("x", &args(&["a\tb"])), "x \"a\tb\"");
        // 纯反斜杠且无空格 → 不用引号，也不转义。
        assert_eq!(command_line("x", &args(&[r"C:\dir\"])), r"x C:\dir\");
    }

    #[test]
    fn environment_block_overrides_case_insensitively_and_ends_with_two_nuls() {
        let block =
            environment_block(&[(String::from("AgeminalTestVar"), String::from("v1"))]).unwrap();
        assert_eq!(block.last(), Some(&0), "末尾要有 NUL");
        assert_eq!(
            block.get(block.len().wrapping_sub(2)),
            Some(&0),
            "必须双 NUL 结尾"
        );

        // 再塞一个同名但大小写不同的值：应该只剩后者。
        let overridden = environment_block(&[
            (String::from("AgeminalTestVar"), String::from("v1")),
            (String::from("AGEMINALTESTVAR"), String::from("v2")),
        ])
        .unwrap();
        let pairs = parse_block(&overridden);
        let hits: Vec<_> = pairs
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("AgeminalTestVar"))
            .collect();
        assert_eq!(hits.len(), 1, "同名项必须只剩一个：{hits:?}");
        assert_eq!(hits[0].1, "v2", "后写的覆盖先写的");

        // 排序：按大写 UTF-16 比较。
        let order: Vec<Vec<u16>> = pairs
            .iter()
            .map(|(name, _)| name.to_uppercase().encode_utf16().collect())
            .collect();
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(order, sorted, "环境块必须按名字排序");
    }

    /// 空环境块也要是**两个** NUL（只有一个 NUL 时 Windows 会把块后面的内存当环境继续读）。
    #[test]
    fn an_empty_environment_block_still_terminates_twice() {
        assert_eq!(merge_environment(Vec::new(), &[]), vec![0, 0]);
    }

    /// 把双 NUL 结尾的块拆回 `(名字, 值)`（测试用）。
    fn parse_block(block: &[u16]) -> Vec<(String, String)> {
        let mut pairs = Vec::new();
        let mut start = 0;
        while start < block.len() && block[start] != 0 {
            let end = block[start..]
                .iter()
                .position(|&unit| unit == 0)
                .map(|offset| start + offset)
                .unwrap();
            let line = String::from_utf16_lossy(&block[start..end]);
            if let Some((name, value)) = line.split_once('=') {
                pairs.push((name.to_owned(), value.to_owned()));
            }
            start = end + 1;
        }
        pairs
    }
}
