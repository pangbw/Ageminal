//! 有界输出环形缓冲（issue #72）。
//!
//! daemon 把每个会话最近的一段 PTY 输出留在内存里，按**累计字节偏移**（`seq`）索引。
//! attach 时只要客户端上报的 `last_seq` 还落在窗口内，就能从它之后精确回放；窗口外的旧位置
//! 只能退化为 ANSI 快照（#71）。
//!
//! 语义约定：
//! - `seq` 是会话内**单调递增的字节偏移**，不是帧号；
//! - 缓冲区只保存连续区间 `[start_seq, end_seq)`；
//! - 溢出时丢最旧的字节，并同步推进 `start_seq`——绝不留下中间空洞。

use std::collections::VecDeque;

/// 一个按绝对偏移索引的字节环形窗口。
#[derive(Debug)]
pub struct RingBuffer {
    capacity: usize,
    start_seq: u64,
    bytes: VecDeque<u8>,
}

impl RingBuffer {
    /// 建一个最多保存 `capacity` 字节的窗口。
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            start_seq: 0,
            bytes: VecDeque::new(),
        }
    }

    /// 当前窗口起点（绝对偏移）。
    #[cfg(test)]
    pub fn start_seq(&self) -> u64 {
        self.start_seq
    }

    /// 当前窗口终点（绝对偏移，等于下一个字节的 `seq`）。
    pub fn end_seq(&self) -> u64 {
        self.start_seq + self.bytes.len() as u64
    }

    /// 当前保存的字节数。
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// 追加一段连续字节；`seq` 必须正好接在当前窗口末尾。
    pub fn push(&mut self, seq: u64, data: &[u8]) {
        assert_eq!(seq, self.end_seq(), "环形缓冲只能按连续偏移追加");
        if self.capacity == 0 || data.is_empty() {
            return;
        }

        if data.len() >= self.capacity {
            // 单段就超过容量：只留最后 capacity 字节。
            let keep_from = data.len() - self.capacity;
            self.bytes.clear();
            self.bytes.extend(&data[keep_from..]);
            self.start_seq = seq + keep_from as u64;
            return;
        }

        self.bytes.extend(data);
        let overflow = self.bytes.len().saturating_sub(self.capacity);
        if overflow > 0 {
            self.bytes.drain(..overflow);
            self.start_seq += overflow as u64;
        }
    }

    /// 取出从 `seq` 到窗口末尾的字节；`seq` 不在窗口内（或超前）返回 `None`。
    ///
    /// `seq == end_seq()` 返回空 `Vec`：调用方表示「已经是最新进度」。
    pub fn since(&self, seq: u64) -> Option<Vec<u8>> {
        if seq < self.start_seq || seq > self.end_seq() {
            return None;
        }
        let skip = (seq - self.start_seq) as usize;
        Some(self.bytes.iter().skip(skip).copied().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_and_reads_by_absolute_offset() {
        let mut ring = RingBuffer::new(16);
        ring.push(0, b"abcdef");
        ring.push(6, b"ghij");

        assert_eq!(ring.start_seq(), 0);
        assert_eq!(ring.end_seq(), 10);
        assert_eq!(ring.since(0).unwrap(), b"abcdefghij");
        assert_eq!(ring.since(3).unwrap(), b"defghij");
        assert!(ring.since(10).unwrap().is_empty());
        assert!(ring.since(11).is_none());
    }

    #[test]
    fn drops_the_oldest_bytes_without_gaps() {
        let mut ring = RingBuffer::new(8);
        ring.push(0, b"abcdef");
        ring.push(6, b"ghijkl");

        assert_eq!(ring.start_seq(), 4, "溢出后应丢掉最旧的 4 字节");
        assert_eq!(ring.end_seq(), 12);
        assert_eq!(ring.since(4).unwrap(), b"efghijkl");
        assert!(ring.since(3).is_none(), "窗口外的旧位置必须报告缺口");
    }

    #[test]
    fn a_single_oversized_chunk_keeps_only_its_tail() {
        let mut ring = RingBuffer::new(4);
        ring.push(0, b"0123456789");

        assert_eq!(ring.start_seq(), 6);
        assert_eq!(ring.end_seq(), 10);
        assert_eq!(ring.since(6).unwrap(), b"6789");
        assert!(ring.since(5).is_none());
    }

    #[test]
    fn zero_capacity_is_explicitly_empty() {
        let mut ring = RingBuffer::new(0);
        ring.push(0, b"abc");
        assert_eq!(ring.len(), 0);
        assert_eq!(ring.end_seq(), 0);
        assert!(ring.since(0).unwrap().is_empty());
    }

    #[test]
    #[should_panic(expected = "连续偏移")]
    fn rejects_a_non_contiguous_append() {
        let mut ring = RingBuffer::new(8);
        ring.push(0, b"abc");
        ring.push(4, b"def");
    }
}
