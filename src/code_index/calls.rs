//! 文件级调用暂存与持久化。调用点不进入节点属性，也不复制文件路径和导入表。
//! BLOB 中的字符串字典仅在单个文件内构建，避免落盘时再次生成整图 JSON。

use std::collections::HashMap;
use std::sync::Arc;

use crate::types::Result;
use super::err;

#[derive(Debug)]
pub(super) struct StoredCall {
    pub source_qn: Arc<str>,
    pub callee_display: Arc<str>,
    pub name: Arc<str>,
    pub scope: Arc<str>,
}

#[derive(Debug, Default)]
pub(super) struct FileCalls {
    pub imports: Vec<String>,
    pub calls: Vec<StoredCall>,
}

impl FileCalls {
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut dictionary = HashMap::<&str, u32>::new();
        let mut strings = Vec::new();
        fn intern<'a>(dictionary: &mut HashMap<&'a str, u32>, strings: &mut Vec<&'a str>, value: &'a str) -> Result<u32> {
            if let Some(id) = dictionary.get(value) { return Ok(*id); }
            let id = u32::try_from(strings.len()).map_err(|_| err("调用记录字符串过多"))?;
            dictionary.insert(value, id);
            strings.push(value);
            Ok(id)
        }
        let imports = self.imports.iter().map(|value| intern(&mut dictionary, &mut strings, value)).collect::<Result<Vec<_>>>()?;
        let calls = self.calls.iter().map(|call| Ok([
            intern(&mut dictionary, &mut strings, &call.source_qn)?, intern(&mut dictionary, &mut strings, &call.callee_display)?, intern(&mut dictionary, &mut strings, &call.name)?, intern(&mut dictionary, &mut strings, &call.scope)?,
        ])).collect::<Result<Vec<_>>>()?;
        let mut bytes = b"KCALL1".to_vec();
        let put = |bytes: &mut Vec<u8>, value: usize| -> Result<()> {
            bytes.extend_from_slice(&u32::try_from(value).map_err(|_| err("调用记录过大"))?.to_le_bytes());
            Ok(())
        };
        put(&mut bytes, strings.len())?;
        for string in strings { put(&mut bytes, string.len())?; bytes.extend_from_slice(string.as_bytes()); }
        put(&mut bytes, imports.len())?;
        for id in imports { bytes.extend_from_slice(&id.to_le_bytes()); }
        put(&mut bytes, calls.len())?;
        for call in calls { for id in call { bytes.extend_from_slice(&id.to_le_bytes()); } }
        Ok(bytes)
    }

    pub fn decode(mut bytes: &[u8]) -> Result<Self> {
        if !bytes.starts_with(b"KCALL1") { return Err(err("调用记录格式不兼容")); }
        bytes = &bytes[6..];
        fn number(bytes: &mut &[u8]) -> Result<usize> {
            let raw = bytes.get(..4).ok_or_else(|| err("调用记录截断"))?;
            let value = u32::from_le_bytes(raw.try_into().unwrap()) as usize;
            *bytes = &bytes[4..];
            Ok(value)
        }
        let count = number(&mut bytes)?;
        if count > bytes.len() / 4 { return Err(err("调用记录字典损坏")); }
        let mut strings = Vec::<Arc<str>>::with_capacity(count);
        for _ in 0..count {
            let len = number(&mut bytes)?;
            let raw = bytes.get(..len).ok_or_else(|| err("调用记录字符串截断"))?;
            strings.push(std::str::from_utf8(raw).map_err(|_| err("调用记录编码损坏"))?.into());
            bytes = &bytes[len..];
        }
        let string = |bytes: &mut &[u8]| -> Result<Arc<str>> {
            strings.get(number(bytes)?).cloned().ok_or_else(|| err("调用记录字典引用损坏"))
        };
        let count = number(&mut bytes)?;
        if count > bytes.len() / 4 { return Err(err("调用记录导入表损坏")); }
        let mut imports = Vec::with_capacity(count);
        for _ in 0..count { imports.push(string(&mut bytes)?.to_string()); }
        let count = number(&mut bytes)?;
        if count > bytes.len() / 16 { return Err(err("调用记录列表损坏")); }
        let mut calls = Vec::with_capacity(count);
        for _ in 0..count {
            calls.push(StoredCall { source_qn: string(&mut bytes)?, callee_display: string(&mut bytes)?,
                name: string(&mut bytes)?, scope: string(&mut bytes)? });
        }
        if !bytes.is_empty() { return Err(err("调用记录存在多余数据")); }
        Ok(Self { imports, calls })
    }
}
