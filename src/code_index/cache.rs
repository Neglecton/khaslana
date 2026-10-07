//! 查询图按持久化代际缓存，避免每个 AI 工具调用都重新加载节点和调用邻接表。

use std::num::NonZeroUsize;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use lru::LruCache;

use super::err;
use super::graph::{GraphBuffer, NodeId};
use super::queries::CallAdjacency;
use super::store::open_read_only_if_exists;
use crate::types::Result;

pub(super) struct CachedIndex {
    pub graph: GraphBuffer,
    pub adjacency: CallAdjacency,
    pub names: std::collections::HashMap<String, Vec<NodeId>>,
    pub file_hashes: std::collections::HashMap<String, (u64, u64)>,
    bytes: usize,
}

impl Deref for CachedIndex {
    type Target = GraphBuffer;
    fn deref(&self) -> &GraphBuffer {
        &self.graph
    }
}

impl CachedIndex {
    fn new(graph: GraphBuffer, hashes: Vec<super::store::FileHashRow>) -> Self {
        let adjacency = CallAdjacency::build(&graph);
        let mut names = std::collections::HashMap::<String, Vec<NodeId>>::new();
        let mut bytes = 0;
        for node in &graph.nodes {
            names.entry(node.name.clone()).or_default().push(node.id);
            bytes += std::mem::size_of_val(node)
                + node.name.len() * 2
                + node.qualified_name.len()
                + node.file_path.len()
                + node.properties.len()
                + 32;
        }
        for edge in &graph.edges {
            bytes += std::mem::size_of_val(edge) + edge.properties.len() + 48;
        }
        bytes += hashes
            .iter()
            .map(|row| row.rel_path.len() + 64)
            .sum::<usize>();
        let file_hashes = hashes
            .into_iter()
            .map(|row| (row.rel_path, (row.mtime_ns, row.size)))
            .collect();
        Self {
            graph,
            adjacency,
            names,
            file_hashes,
            bytes,
        }
    }
}

type CacheKey = (PathBuf, String);
static CACHE: OnceLock<Mutex<LruCache<CacheKey, Arc<CachedIndex>>>> = OnceLock::new();
const CACHE_BYTES: usize = 64 * 1024 * 1024;

pub(super) fn load_index(path: &Path) -> Result<Arc<CachedIndex>> {
    let store = open_read_only_if_exists(path)?
        .ok_or_else(|| err("代码索引不存在或尚未建立，请先调用 refresh_index"))?;
    // 代际和图必须来自同一读取快照，不能把刷新后的图放到旧代际键下。
    let tx = store
        .conn
        .unchecked_transaction()
        .map_err(|e| err(format!("开启索引查询失败：{e}")))?;
    let generation = tx
        .query_row(
            "SELECT value FROM meta WHERE key = 'generation'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok();
    let cache = CACHE.get_or_init(|| Mutex::new(LruCache::new(NonZeroUsize::new(8).unwrap())));
    let key = generation.map(|generation| {
        (
            path.canonicalize().unwrap_or_else(|_| path.to_path_buf()),
            generation,
        )
    });
    if let Some(key) = &key {
        if let Some(index) = cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .cloned()
        {
            tx.commit()
                .map_err(|e| err(format!("结束索引查询失败：{e}")))?;
            return Ok(index);
        }
    }
    let index = Arc::new(CachedIndex::new(
        store.load_graph()?,
        store.load_file_hashes()?,
    ));
    tx.commit()
        .map_err(|e| err(format!("结束索引查询失败：{e}")))?;
    if let Some(key) = key.filter(|_| index.bytes <= CACHE_BYTES) {
        let mut entries = cache.lock().unwrap_or_else(|e| e.into_inner());
        let obsolete: Vec<_> = entries
            .iter()
            .filter(|(cached_key, _)| cached_key.0 == key.0 && **cached_key != key)
            .map(|(cached_key, _)| cached_key.clone())
            .collect();
        for cached_key in obsolete {
            entries.pop(&cached_key);
        }
        entries.put(key, Arc::clone(&index));
        while entries.iter().map(|(_, graph)| graph.bytes).sum::<usize>() > CACHE_BYTES {
            entries.pop_lru();
        }
    }
    Ok(index)
}
