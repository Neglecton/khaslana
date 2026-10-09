//! 有界 stdio 传输；换行 JSON 与兼容的 Content-Length 帧逐帧识别。

use std::io::{self, BufRead, Read, Write};
use serde_json::Value;
use super::{McpServer, MAX_FRAME_BYTES, jsonrpc_error};

const MAX_HEADER_BYTES: usize = 16 * 1024;

struct Frame {
    bytes: Vec<u8>,
    content_length: bool,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn bounded_line(reader: &mut impl BufRead, limit: usize) -> io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    let count = reader.take(limit as u64 + 1).read_until(b'\n', &mut bytes)?;
    if count > limit { return Err(invalid("MCP 帧或头部超过字节上限")); }
    Ok((count > 0).then_some(bytes))
}

fn read_frame(reader: &mut impl BufRead) -> io::Result<Option<Frame>> {
    loop {
        let Some(line) = bounded_line(reader, MAX_FRAME_BYTES)? else { return Ok(None); };
        if line.iter().all(u8::is_ascii_whitespace) { continue; }
        if !line.get(..15).is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"content-length:")) {
            return Ok(Some(Frame { bytes: line, content_length: false }));
        }
        // 长度无效时关闭连接，不把无法确定边界的正文误当下一条请求。
        let header = std::str::from_utf8(&line).map_err(|_| invalid("MCP 头部不是 UTF-8"))?;
        let length: usize = header[15..].trim().parse().map_err(|_| invalid("Content-Length 无效"))?;
        if length > MAX_FRAME_BYTES { return Err(invalid("MCP 正文超过字节上限")); }
        let mut header_bytes = line.len();
        loop {
            let budget = MAX_HEADER_BYTES.checked_sub(header_bytes).ok_or_else(|| invalid("MCP 头部超过字节上限"))?;
            let line = bounded_line(reader, budget)?.ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
            header_bytes += line.len();
            let header = std::str::from_utf8(&line).map_err(|_| invalid("MCP 头部不是 UTF-8"))?;
            if header.trim().is_empty() { break; }
            let (name, _) = header.split_once(':').ok_or_else(|| invalid("MCP 头部格式无效"))?;
            if name.trim().eq_ignore_ascii_case("content-length") { return Err(invalid("重复的 Content-Length")); }
        }
        let mut bytes = vec![0; length];
        reader.read_exact(&mut bytes)?;
        return Ok(Some(Frame { bytes, content_length: true }));
    }
}

/// 把 I/O 与进程入口分开，以真实字节流验证帧边界、错误恢复和断开行为。
pub(super) fn serve(server: &McpServer, reader: &mut impl BufRead, writer: &mut impl Write) -> io::Result<()> {
    while let Some(frame) = read_frame(reader)? {
        let response = match std::str::from_utf8(&frame.bytes) {
            Err(_) => Some(jsonrpc_error(&Value::Null, -32700, "Parse error: 消息必须是 UTF-8")),
            Ok(line) => match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| server.handle_message(line))) {
                Ok(response) => response,
                Err(_) => {
                    eprintln!("[khaslana-mcp] 消息处理异常");
                    // 合法通知即使异常也不产生响应；请求保留原始 id。
                    serde_json::from_str::<Value>(line).ok().and_then(|value| value.get("id").cloned())
                        .map(|id| jsonrpc_error(&id, -32603, "Internal error: 工具调用异常"))
                }
            },
        };
        if let Some(response) = response {
            if frame.content_length { write!(writer, "Content-Length: {}\r\n\r\n{response}", response.len())?; }
            else { writeln!(writer, "{response}")?; }
            writer.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../tests/code_index_mcp_transport.rs"]
mod tests;
