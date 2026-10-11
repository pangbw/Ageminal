//! 已打开预览文件的变更监听（#61 验收 1）。
//!
//! 真 watcher + 临时目录：只回推**被监听文件**的变更、忽略同目录的其它文件、把突发写合并。

mod common;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

use ageminal_files::watch::FileWatcher;
use common::{write, TempDir};

/// 收一批变更；超时返回 `None`。
fn recv_batch(rx: &Receiver<Vec<PathBuf>>, timeout: Duration) -> Option<Vec<PathBuf>> {
    match rx.recv_timeout(timeout) {
        Ok(batch) => Some(batch),
        Err(RecvTimeoutError::Timeout) => None,
        Err(RecvTimeoutError::Disconnected) => panic!("监听线程已断开"),
    }
}

/// 起一个监听器，返回它、变更通道与两个文件路径。
fn watcher(tag: &str) -> (FileWatcher, Receiver<Vec<PathBuf>>, PathBuf, PathBuf) {
    let dir = TempDir::new(tag);
    let watched = dir.path().join("watched.txt");
    let sibling = dir.path().join("sibling.txt");
    write(dir.path(), "watched.txt", "one\n");
    write(dir.path(), "sibling.txt", "one\n");

    let (tx, rx) = mpsc::channel();
    let watcher = FileWatcher::start(move |paths| {
        let _ = tx.send(paths);
    })
    .expect("创建监听器");
    watcher
        .set_files(std::slice::from_ref(&watched))
        .expect("设置监听集合");

    // 夹具目录随 `dir` 一起被删；把 dir 的所有权泄漏给监听器生命周期即可——测试进程本就要退出。
    std::mem::forget(dir);
    (watcher, rx, watched, sibling)
}

#[test]
fn reports_a_change_to_a_watched_file() {
    let (watcher, rx, watched, _sibling) = watcher("watch-hit");

    std::fs::write(&watched, "two\n").unwrap();

    let batch = recv_batch(&rx, Duration::from_secs(3)).expect("应收到变更");
    assert!(batch.contains(&watched), "批次应含被监听文件：{batch:?}");

    drop(watcher);
}

#[test]
fn ignores_changes_outside_the_watch_set() {
    let (watcher, rx, _watched, sibling) = watcher("watch-miss");

    std::fs::write(&sibling, "two\n").unwrap();

    assert!(
        recv_batch(&rx, Duration::from_millis(500)).is_none(),
        "未被监听的文件不该回推"
    );

    drop(watcher);
}

#[test]
fn coalesces_a_burst_into_one_batch() {
    let (watcher, rx, watched, _sibling) = watcher("watch-burst");

    for body in ["a\n", "ab\n", "abc\n", "abcd\n"] {
        std::fs::write(&watched, body).unwrap();
        std::thread::sleep(Duration::from_millis(10));
    }

    let batch = recv_batch(&rx, Duration::from_secs(3)).expect("应收到变更");
    assert!(batch.contains(&watched));
    // 去抖窗口内的突发写合并成一批；窗口过后不该再有第二批。
    assert!(
        recv_batch(&rx, Duration::from_millis(500)).is_none(),
        "突发写应合并成一批"
    );

    drop(watcher);
}
