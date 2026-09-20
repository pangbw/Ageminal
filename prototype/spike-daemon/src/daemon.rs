//! Named Pipe 服务端（B2 #5 的单连接 + 控制优先队列 + 快照回放）。

use crate::proto::*;
use crate::session::Session;
use std::io;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::{broadcast, mpsc};

pub async fn serve(pipe_name: String, s: Arc<Session>) -> io::Result<()> {
    let mut first = true;
    loop {
        if s.shutdown.load(Ordering::SeqCst) {
            return Ok(());
        }
        let mut opts = ServerOptions::new();
        if first {
            // 单例：重复创建会 PermissionDenied（B4 #7）
            opts.first_pipe_instance(true);
            first = false;
        }
        let server = opts.create(&pipe_name)?;
        crate::logbuf::log(&format!("管道已创建，等待客户端：{pipe_name}"));
        server.connect().await?;
        crate::logbuf::log("客户端已连接");
        let s2 = Arc::clone(&s);
        tokio::spawn(async move {
            if let Err(e) = handle(server, s2).await {
                eprintln!("[daemon] client 结束：{e}");
            }
        });
    }
}

async fn handle(pipe: NamedPipeServer, s: Arc<Session>) -> io::Result<()> {
    let (mut rd, mut wr) = tokio::io::split(pipe);
    // ⚠️ 先订阅再快照，避免"订阅前丢字节"
    let mut rx = s.live.subscribe();

    let (kind, _p) = tokio::time::timeout(std::time::Duration::from_secs(5), read_frame(&mut rd))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "hello timeout"))??;
    if kind != KIND_HELLO {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "expected HELLO"));
    }

    // 控制帧优先队列
    let (ctx, mut crx) = mpsc::channel::<Vec<u8>>(4096);

    let mut ack = Vec::new();
    ack.extend_from_slice(&PROTO_VERSION.to_le_bytes());
    ack.extend_from_slice(&(CAP_REPLAY | CAP_RESIZE | CAP_PING).to_le_bytes());
    ack.extend_from_slice(&0u64.to_le_bytes());
    ack.extend_from_slice(&s.pty.pid().to_le_bytes());
    ack.extend_from_slice(&(s.pty.alive() as u32).to_le_bytes());
    let _ = ctx.send(frame(KIND_HELLO_ACK, &ack)).await;

    // 快照 + 去重偏移
    let (replay, total) = {
        let r = s.ring.lock().unwrap();
        r.snapshot()
    };
    if !replay.is_empty() {
        let _ = ctx.send(frame(KIND_REPLAY, &replay)).await;
    }

    let s2 = Arc::clone(&s);
    let writer = tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(500));
        loop {
            tokio::select! {
                biased;
                f = crx.recv() => match f {
                    Some(f) => wr.write_all(&f).await?,
                    None => break,
                },
                r = rx.recv() => match r {
                    Ok(c) => {
                        let end = c.start + c.bytes.len() as u64;
                        if end <= total { continue; }
                        let bytes = if c.start < total {
                            c.bytes[(total - c.start) as usize..].to_vec()
                        } else {
                            c.bytes
                        };
                        wr.write_all(&frame(KIND_DATA, &bytes)).await?;
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        s2.gaps.fetch_add(1, Ordering::Relaxed);
                        wr.write_all(&frame(KIND_GAP, &n.to_le_bytes())).await?;
                    }
                    Err(_) => break,
                },
                _ = tick.tick() => {
                    if !s2.reader_alive.load(Ordering::SeqCst) {
                        let _ = wr.write_all(&frame(KIND_STATUS_REPLY, b"{\"exited\":true}")).await;
                        break;
                    }
                }
            }
        }
        Ok::<(), io::Error>(())
    });

    loop {
        let (kind, payload) = match read_frame(&mut rd).await {
            Ok(v) => v,
            Err(_) => break,
        };
        match kind {
            KIND_INPUT => {
                let _ = s.write_input(&payload);
            }
            KIND_RESIZE => {
                if payload.len() >= 4 {
                    let (c, r) = u16s(&payload);
                    let _ = s.pty.resize(c, r);
                }
            }
            KIND_PING => {
                let _ = ctx.send(frame(KIND_PONG, &payload)).await;
            }
            KIND_STATUS => {
                let _ = ctx
                    .send(frame(KIND_STATUS_REPLY, s.status_json().as_bytes()))
                    .await;
            }
            KIND_DETACH => break,
            KIND_SHUTDOWN => {
                crate::logbuf::log("收到 SHUTDOWN");
                s.shutdown.store(true, Ordering::SeqCst);
                break;
            }
            _ => {}
        }
    }
    drop(ctx);
    let _ = writer.await;
    Ok(())
}
