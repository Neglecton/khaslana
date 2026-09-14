use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub const MAX_LSP_FRAME_BYTES: usize = 8 * 1024 * 1024;
const MAX_HEADER_BYTES: usize = 64 * 1024;

#[derive(Debug, Error)]
pub enum FrameError {
    #[error("LSP I/O 错误：{0}")]
    Io(#[from] io::Error),
    #[error("LSP 帧头无效：{0}")]
    InvalidHeader(String),
    #[error("LSP 帧缺少 Content-Length")]
    MissingContentLength,
    #[error("LSP 帧超过 {limit} 字节上限：{actual}")]
    FrameTooLarge { actual: usize, limit: usize },
    #[error("LSP JSON 无效：{0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CharacterEncoding {
    Utf8,
    #[default]
    Utf16,
    Utf32,
}

impl CharacterEncoding {
    pub fn from_server(value: Option<&str>) -> Self {
        match value {
            Some("utf-8") => Self::Utf8,
            Some("utf-32") => Self::Utf32,
            _ => Self::Utf16,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LspPosition {
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LspRange {
    pub start: LspPosition,
    pub end: LspPosition,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspLocation {
    pub uri: String,
    pub range: LspRange,
}

/// 读取一个 Content-Length 帧。干净 EOF 返回 `Ok(None)`；半截帧返回 I/O 错误。
pub fn read_lsp_frame<R: BufRead>(reader: &mut R) -> Result<Option<Value>, FrameError> {
    let mut content_length = None;
    let mut header_bytes = 0usize;
    loop {
        let mut line = Vec::new();
        let read = reader.read_until(b'\n', &mut line)?;
        if read == 0 {
            return if header_bytes == 0 {
                Ok(None)
            } else {
                Err(FrameError::InvalidHeader("帧头在空行前结束".to_string()))
            };
        }
        header_bytes = header_bytes.saturating_add(read);
        if header_bytes > MAX_HEADER_BYTES {
            return Err(FrameError::InvalidHeader("帧头过大".to_string()));
        }
        if line == b"\r\n" || line == b"\n" {
            break;
        }
        let text = std::str::from_utf8(&line)
            .map_err(|_| FrameError::InvalidHeader("帧头不是 UTF-8".to_string()))?
            .trim_end_matches(['\r', '\n']);
        let Some((name, raw_value)) = text.split_once(':') else {
            return Err(FrameError::InvalidHeader(text.to_string()));
        };
        if name.eq_ignore_ascii_case("Content-Length") {
            if content_length.is_some() {
                return Err(FrameError::InvalidHeader(
                    "重复的 Content-Length".to_string(),
                ));
            }
            let length = raw_value
                .trim()
                .parse::<usize>()
                .map_err(|_| FrameError::InvalidHeader(text.to_string()))?;
            if length > MAX_LSP_FRAME_BYTES {
                return Err(FrameError::FrameTooLarge {
                    actual: length,
                    limit: MAX_LSP_FRAME_BYTES,
                });
            }
            content_length = Some(length);
        }
    }
    let length = content_length.ok_or(FrameError::MissingContentLength)?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    Ok(Some(serde_json::from_slice(&body)?))
}

pub fn write_lsp_frame<W: Write>(writer: &mut W, value: &Value) -> Result<(), FrameError> {
    let body = serde_json::to_vec(value)?;
    if body.len() > MAX_LSP_FRAME_BYTES {
        return Err(FrameError::FrameTooLarge {
            actual: body.len(),
            limit: MAX_LSP_FRAME_BYTES,
        });
    }
    write!(writer, "Content-Length: {}\r\n\r\n", body.len())?;
    writer.write_all(&body)?;
    writer.flush()?;
    Ok(())
}

pub fn scalar_position_to_lsp(
    text: &str,
    line_one_based: u32,
    column_one_based: u32,
    encoding: CharacterEncoding,
) -> Result<LspPosition, String> {
    if line_one_based == 0 || column_one_based == 0 {
        return Err("源码行列从 1 开始".to_string());
    }
    let line = source_line(text, line_one_based)?;
    let scalar_index = usize::try_from(column_one_based - 1).map_err(|_| "列号过大")?;
    let byte_in_line = scalar_to_byte(line, scalar_index)?;
    let prefix = &line[..byte_in_line];
    let character = match encoding {
        CharacterEncoding::Utf8 => prefix.len(),
        CharacterEncoding::Utf16 => prefix.encode_utf16().count(),
        CharacterEncoding::Utf32 => prefix.chars().count(),
    };
    Ok(LspPosition {
        line: line_one_based - 1,
        character: u32::try_from(character).map_err(|_| "列号过大")?,
    })
}

pub fn lsp_position_to_byte_offset(
    text: &str,
    position: LspPosition,
    encoding: CharacterEncoding,
) -> Result<usize, String> {
    let (line_start, line) = source_line_with_offset(text, position.line + 1)?;
    let target = usize::try_from(position.character).map_err(|_| "LSP 列号过大")?;
    let mut units = 0usize;
    for (byte, ch) in line.char_indices() {
        if units == target {
            return Ok(line_start + byte);
        }
        units += match encoding {
            CharacterEncoding::Utf8 => ch.len_utf8(),
            CharacterEncoding::Utf16 => ch.len_utf16(),
            CharacterEncoding::Utf32 => 1,
        };
        if units > target {
            return Err("LSP 位置落在字符编码单元中间".to_string());
        }
    }
    if units == target {
        Ok(line_start + line.len())
    } else {
        Err("LSP 列号超出行末".to_string())
    }
}

pub fn byte_offset_to_lsp_position(
    text: &str,
    byte_offset: usize,
    encoding: CharacterEncoding,
) -> Result<LspPosition, String> {
    if byte_offset > text.len() || !text.is_char_boundary(byte_offset) {
        return Err("字节位置不是有效 UTF-8 边界".to_string());
    }
    let prefix = &text[..byte_offset];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count();
    let line_start = prefix.rfind('\n').map_or(0, |index| index + 1);
    let column_text = &text[line_start..byte_offset];
    let character = match encoding {
        CharacterEncoding::Utf8 => column_text.len(),
        CharacterEncoding::Utf16 => column_text.encode_utf16().count(),
        CharacterEncoding::Utf32 => column_text.chars().count(),
    };
    Ok(LspPosition {
        line: u32::try_from(line).map_err(|_| "行号过大")?,
        character: u32::try_from(character).map_err(|_| "列号过大")?,
    })
}

fn source_line(text: &str, line_one_based: u32) -> Result<&str, String> {
    source_line_with_offset(text, line_one_based).map(|(_, line)| line)
}

fn source_line_with_offset(text: &str, line_one_based: u32) -> Result<(usize, &str), String> {
    if line_one_based == 0 {
        return Err("源码行号从 1 开始".to_string());
    }
    let mut start = 0usize;
    let target = usize::try_from(line_one_based - 1).map_err(|_| "行号过大")?;
    for (index, raw) in text.split_inclusive('\n').enumerate() {
        if index == target {
            let without_lf = raw.strip_suffix('\n').unwrap_or(raw);
            return Ok((start, without_lf.strip_suffix('\r').unwrap_or(without_lf)));
        }
        start += raw.len();
    }
    if target == text.bytes().filter(|byte| *byte == b'\n').count() && text.ends_with('\n') {
        return Ok((text.len(), ""));
    }
    Err("源码行号超出文件范围".to_string())
}

fn scalar_to_byte(line: &str, scalar_index: usize) -> Result<usize, String> {
    if scalar_index == line.chars().count() {
        return Ok(line.len());
    }
    line.char_indices()
        .nth(scalar_index)
        .map(|(index, _)| index)
        .ok_or_else(|| "源码列号超出行末".to_string())
}

pub fn path_to_file_uri(path: &Path) -> Result<String, String> {
    let absolute = std::fs::canonicalize(path).map_err(|error| format!("路径无效：{error}"))?;
    absolute_path_to_file_uri(&absolute)
}

pub(crate) fn absolute_path_to_file_uri(path: &Path) -> Result<String, String> {
    if !path.is_absolute() {
        return Err("file URI 必须由绝对路径生成".to_string());
    }
    let mut normalized = path.to_string_lossy().replace('\\', "/");
    // Windows canonicalize 会返回 `\\?\C:\...`。扩展路径前缀是 Win32 API 细节，
    // 不能放进 file URI，否则 Java 会把 `?` 解析成一个无主机的 authority。
    if let Some(unc) = normalized.strip_prefix("//?/UNC/") {
        normalized = format!("//{unc}");
    } else if let Some(local) = normalized.strip_prefix("//?/") {
        normalized = local.to_string();
    }
    let prefix = if normalized.starts_with("//") {
        "file:"
    } else if normalized.starts_with('/') {
        "file://"
    } else {
        "file:///"
    };
    Ok(format!("{prefix}{}", percent_encode_path(&normalized)))
}

pub fn file_uri_to_path(uri: &str) -> Result<PathBuf, String> {
    let raw = uri
        .strip_prefix("file://")
        .ok_or_else(|| "只接受 file URI".to_string())?;
    let decoded = percent_decode(raw)?;
    #[cfg(windows)]
    let decoded = decoded
        .strip_prefix('/')
        .filter(|value| value.as_bytes().get(1) == Some(&b':'))
        .unwrap_or(&decoded)
        .to_string();
    Ok(PathBuf::from(decoded))
}

fn percent_encode_path(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b':' | b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn percent_decode(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = bytes
                .get(index + 1..index + 3)
                .ok_or("URI 百分号编码不完整")?;
            let text = std::str::from_utf8(hex).map_err(|_| "URI 百分号编码无效")?;
            out.push(u8::from_str_radix(text, 16).map_err(|_| "URI 百分号编码无效")?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).map_err(|_| "URI 解码后不是 UTF-8".to_string())
}
