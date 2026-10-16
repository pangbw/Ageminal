//! 发送侧的**控制优先队列**（研究稿 §五 做法 B）。
//!
//! 为什么需要一个专门的队列：研究稿记了 WezTerm mux-server 的真实翻车——「每连接无界队列 +
//! 单一调度线程」会让 `GetCodecVersion` / `ListPanes` / 新 attach 在持续 `PaneOutput` 下**饿死
//! 到超时**。所以控制面与数据面分成两条队列，出队时**控制面优先**。
//!
//! 这里只管优先级；「在途数据字节有上限 / 洪峰时合并」（研究稿 §五 的另一条）是流控，见 #73。

use std::collections::VecDeque;

use crate::frame::Frame;

/// 出站队列：**控制帧永远先于数据帧**出队。
#[derive(Debug, Default)]
pub struct ControlFirstQueue {
    control: VecDeque<Frame>,
    data: VecDeque<Frame>,
    queued_data_bytes: usize,
}

impl ControlFirstQueue {
    /// 建一个空队列。
    pub fn new() -> Self {
        Self::default()
    }

    /// 入队——按帧类型自动分流（`Data` / `Input` 走数据面，其余走控制面）。
    pub fn push(&mut self, frame: Frame) {
        if frame.kind.is_raw_payload() {
            self.queued_data_bytes += frame.encoded_len();
            self.data.push_back(frame);
        } else {
            self.control.push_back(frame);
        }
    }

    /// 出队：**控制面清空之前，一个数据帧都不出去**；同类内部保持先进先出。
    pub fn pop(&mut self) -> Option<Frame> {
        if let Some(frame) = self.control.pop_front() {
            return Some(frame);
        }
        let frame = self.data.pop_front()?;
        self.queued_data_bytes -= frame.encoded_len();
        Some(frame)
    }

    /// 是否已空。
    pub fn is_empty(&self) -> bool {
        self.control.is_empty() && self.data.is_empty()
    }

    /// 在队帧总数。
    pub fn len(&self) -> usize {
        self.control.len() + self.data.len()
    }

    /// **在途数据字节**（含帧头）——#73 流控高低水位的输入。
    pub fn queued_data_bytes(&self) -> usize {
        self.queued_data_bytes
    }

    /// 待发控制帧数。
    pub fn pending_control(&self) -> usize {
        self.control.len()
    }

    /// 待发数据帧数。
    pub fn pending_data(&self) -> usize {
        self.data.len()
    }
}

#[cfg(test)]
mod tests {
    use crate::frame::{Frame, FrameKind};

    use super::*;

    fn data(seq: u64) -> Frame {
        Frame::raw(FrameKind::Data, 1, seq, vec![0u8; 8])
    }

    fn resize() -> Frame {
        Frame::control(FrameKind::Resize, 0, 0, b"{}".to_vec())
    }

    /// 验收 2：控制消息优先于数据。
    #[test]
    fn control_always_precedes_data() {
        let mut queue = ControlFirstQueue::new();
        queue.push(data(1));
        queue.push(data(2));
        queue.push(resize());
        queue.push(data(3));
        queue.push(resize());

        assert_eq!(queue.pending_data(), 3);
        assert_eq!(queue.pending_control(), 2);

        // 两条控制帧先出——哪怕它们排在三帧数据后面入队。
        assert_eq!(queue.pop().unwrap().kind, FrameKind::Resize);
        assert_eq!(queue.pop().unwrap().kind, FrameKind::Resize);
        // 然后才轮到数据，且保持先进先出。
        assert_eq!(queue.pop().unwrap().seq, 1);
        assert_eq!(queue.pop().unwrap().seq, 2);
        assert_eq!(queue.pop().unwrap().seq, 3);
        assert!(queue.pop().is_none());
    }

    /// 洪峰下的饥饿测试：连续灌数据，控制帧仍立刻可出。
    #[test]
    fn control_is_never_starved_by_a_data_flood() {
        let mut queue = ControlFirstQueue::new();
        for seq in 0..1000 {
            queue.push(data(seq));
        }
        queue.push(resize());
        assert_eq!(queue.pop().unwrap().kind, FrameKind::Resize);
    }

    #[test]
    fn data_bytes_are_accounted_and_released_on_pop() {
        let mut queue = ControlFirstQueue::new();
        queue.push(data(1));
        let one = queue.queued_data_bytes();
        assert!(one > 0);

        queue.push(data(2));
        assert_eq!(queue.queued_data_bytes(), one * 2);
        assert_eq!(queue.len(), 2);

        queue.pop();
        assert_eq!(queue.queued_data_bytes(), one);
        queue.pop();
        assert_eq!(queue.queued_data_bytes(), 0);
        assert!(queue.is_empty());
    }

    #[test]
    fn input_frames_are_data_plane_too() {
        let mut queue = ControlFirstQueue::new();
        queue.push(Frame::raw(FrameKind::Input, 1, 1, b"x".to_vec()));
        queue.push(resize());
        assert_eq!(queue.pending_data(), 1);
        assert_eq!(queue.pop().unwrap().kind, FrameKind::Resize);
    }
}
