#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictResolutionSide {
    Ours,
    Theirs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictBlockResolution {
    Ours,
    Theirs,
    BothOursFirst,
    BothTheirsFirst,
}

impl ConflictBlockResolution {
    pub fn render(self, ours: &str, theirs: &str) -> String {
        match self {
            Self::Ours => ours.to_string(),
            Self::Theirs => theirs.to_string(),
            Self::BothOursFirst => format!("{ours}{theirs}"),
            Self::BothTheirsFirst => format!("{theirs}{ours}"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictBlockStatus {
    Unresolved,
    Resolved(ConflictBlockResolution),
    Ignored,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictFileKind {
    Text,
    Binary,
    Unsupported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictDraftStatus {
    Clean,
    Dirty,
    Applied,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictBlock {
    pub base: Option<String>,
    pub ours: String,
    pub theirs: String,
    pub start: usize,
    pub end: usize,
    pub ours_start: usize,
    pub ours_end: usize,
    pub theirs_start: usize,
    pub theirs_end: usize,
    pub status: ConflictBlockStatus,
    pub has_manual_edits: bool,
}

impl ConflictBlock {
    pub fn resolved_text(&self, resolution: ConflictBlockResolution) -> String {
        resolution.render(&self.ours, &self.theirs)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictFileView {
    pub path: String,
    pub kind: ConflictFileKind,
    pub draft: String,
    pub ours_text: String,
    pub theirs_text: String,
    pub blocks: Vec<ConflictBlock>,
    pub draft_status: ConflictDraftStatus,
    pub fallback_reason: Option<String>,
}

impl ConflictFileView {
    pub fn unresolved_block_count(&self) -> usize {
        self.blocks
            .iter()
            .filter(|block| matches!(block.status, ConflictBlockStatus::Unresolved))
            .count()
    }

    pub fn ignored_block_count(&self) -> usize {
        self.blocks
            .iter()
            .filter(|block| matches!(block.status, ConflictBlockStatus::Ignored))
            .count()
    }

    pub fn handled_block_count(&self) -> usize {
        self.blocks
            .iter()
            .filter(|block| !matches!(block.status, ConflictBlockStatus::Unresolved))
            .count()
    }

    pub fn requires_resolution_confirmation(&self) -> bool {
        self.unresolved_block_count() > 0
    }

    pub fn has_manual_blocks(&self) -> bool {
        self.blocks.iter().any(|block| block.has_manual_edits)
    }

    pub fn mark_applied(&mut self) {
        self.draft_status = ConflictDraftStatus::Applied;
    }

    pub fn mark_dirty(&mut self) {
        self.draft_status = ConflictDraftStatus::Dirty;
    }

    pub fn apply_block_resolution(
        &mut self,
        block_index: usize,
        resolution: ConflictBlockResolution,
    ) {
        let Some(block) = self.blocks.get(block_index).cloned() else {
            return;
        };
        let replacement = block.resolved_text(resolution);
        self.replace_block_text(
            block_index,
            replacement,
            ConflictBlockStatus::Resolved(resolution),
            false,
        );
    }

    pub fn ignore_block(&mut self, block_index: usize) {
        let Some(block) = self.blocks.get_mut(block_index) else {
            return;
        };
        block.status = ConflictBlockStatus::Ignored;
        block.has_manual_edits = false;
        self.draft_status = ConflictDraftStatus::Dirty;
    }

    pub fn set_draft(&mut self, new_draft: String) {
        if self.draft == new_draft {
            return;
        }

        let old_draft = self.draft.clone();
        let prefix = shared_prefix_len(&old_draft, &new_draft);
        let suffix = shared_suffix_len(&old_draft[prefix..], &new_draft[prefix..]);
        let old_changed_end = old_draft.len().saturating_sub(suffix);
        let new_changed_end = new_draft.len().saturating_sub(suffix);
        let delta = (new_changed_end as isize - prefix as isize)
            - (old_changed_end as isize - prefix as isize);

        for block in &mut self.blocks {
            if block.end <= prefix {
                continue;
            }
            if block.start >= old_changed_end {
                shift_range(block, delta);
                continue;
            }

            block.has_manual_edits = true;
            if block.start > prefix {
                block.start = prefix;
            }
            // 块尾随编辑区长度差平移后可能落在多字节字符中间（例如把 ASCII
            // 中段改成多字节文本），后续按字节 replace_range 会 panic；
            // 吸附到最近的字符边界。
            block.end =
                clamp_to_char_boundary(&new_draft, add_signed(block.end, delta).max(block.start));
            block.status = ConflictBlockStatus::Unresolved;
        }

        self.draft = new_draft;
        self.draft_status = ConflictDraftStatus::Dirty;
    }

    fn replace_block_text(
        &mut self,
        block_index: usize,
        replacement: String,
        status: ConflictBlockStatus,
        manual: bool,
    ) {
        let Some(block) = self.blocks.get(block_index).cloned() else {
            return;
        };

        self.draft
            .replace_range(block.start..block.end, &replacement);
        let delta = replacement.len() as isize - (block.end - block.start) as isize;
        if let Some(current) = self.blocks.get_mut(block_index) {
            current.end = current.start + replacement.len();
            current.status = status;
            current.has_manual_edits = manual;
        }
        for later in self.blocks.iter_mut().skip(block_index + 1) {
            shift_range(later, delta);
        }
        self.draft_status = ConflictDraftStatus::Dirty;
    }
}

fn shift_range(block: &mut ConflictBlock, delta: isize) {
    block.start = add_signed(block.start, delta);
    block.end = add_signed(block.end, delta).max(block.start);
}

fn add_signed(value: usize, delta: isize) -> usize {
    if delta >= 0 {
        value.saturating_add(delta as usize)
    } else {
        value.saturating_sub(delta.unsigned_abs())
    }
}

/// 把字节偏移吸附到不超过它的最近 UTF-8 字符边界。
/// 越界或本身就在边界上时原样返回（钳制到文本长度）。
fn clamp_to_char_boundary(text: &str, mut offset: usize) -> usize {
    if offset >= text.len() {
        return text.len();
    }
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn shared_prefix_len(left: &str, right: &str) -> usize {
    let mut prefix = 0;
    let mut left_iter = left.char_indices();
    let mut right_iter = right.char_indices();
    loop {
        match (left_iter.next(), right_iter.next()) {
            (Some((left_index, left_ch)), Some((right_index, right_ch)))
                if left_index == prefix && right_index == prefix && left_ch == right_ch =>
            {
                prefix = left_index + left_ch.len_utf8();
            }
            _ => break,
        }
    }
    prefix
}

fn shared_suffix_len(left: &str, right: &str) -> usize {
    let mut suffix = 0;
    let mut left_iter = left.chars().rev();
    let mut right_iter = right.chars().rev();
    loop {
        match (left_iter.next(), right_iter.next()) {
            (Some(left_ch), Some(right_ch)) if left_ch == right_ch => {
                suffix += left_ch.len_utf8();
            }
            _ => break,
        }
    }
    suffix
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_draft_clamps_block_end_to_char_boundary() {
        let mut view = ConflictFileView {
            path: "f".into(),
            kind: ConflictFileKind::Text,
            draft: "XabY".into(),
            ours_text: String::new(),
            theirs_text: String::new(),
            blocks: vec![ConflictBlock {
                base: None,
                ours: "Xa".into(),
                theirs: String::new(),
                start: 0,
                end: 2,
                ours_start: 0,
                ours_end: 2,
                theirs_start: 0,
                theirs_end: 2,
                status: ConflictBlockStatus::Unresolved,
                has_manual_edits: false,
            }],
            draft_status: ConflictDraftStatus::Clean,
            fallback_reason: None,
        };

        // 把编辑区内的 ASCII 段替换为多字节字符：块尾平移 +1 后落在“中”
        // （3 字节）内部，必须吸附到字符边界，否则后续按字节 replace_range
        // 会 panic。
        view.set_draft("X中Y".into());

        assert_eq!(view.draft, "X中Y");
        assert!(view.draft.is_char_boundary(view.blocks[0].start));
        assert!(view.draft.is_char_boundary(view.blocks[0].end));
        assert!(view.blocks[0].start <= view.blocks[0].end);
    }

    #[test]
    fn clamp_to_char_boundary_snaps_backwards() {
        // “中”占 1..4，字节 3 在其内部，应吸附到 1。
        assert_eq!(clamp_to_char_boundary("a中b", 3), 1);
        assert_eq!(clamp_to_char_boundary("a中b", 4), 4);
        assert_eq!(clamp_to_char_boundary("a中b", 99), 5);
        assert_eq!(clamp_to_char_boundary("a中b", 0), 0);
    }
}
