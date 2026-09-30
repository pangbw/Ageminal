//! app-core ↔ daemon 的帧协议（Rust 单一来源，见 REQUIREMENTS.md §19）。
//!
//! 本阶段（issue #50）只放协议版本这一最小契约；控制帧 / 数据帧的编解码
//! 在 issue #65 落地。

/// 协议版本。与 app 版本解耦，仅在帧形状变化时递增。
pub const PROTOCOL_VERSION: u32 = 1;

#[cfg(test)]
mod tests {
    #[test]
    fn protocol_version_starts_at_one() {
        assert_eq!(super::PROTOCOL_VERSION, 1);
    }
}
