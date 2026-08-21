use super::*;

#[test]
fn change_sections_compact_when_both_sides_are_empty() {
    assert_eq!(
        change_sections_layout(0, false, 0, false),
        ChangeSectionsLayout {
            staged: ChangeSectionHeight::Compact,
            unstaged: ChangeSectionHeight::Compact,
        }
    );
}

#[test]
fn change_sections_share_remaining_height_when_both_sides_have_content() {
    assert_eq!(
        change_sections_layout(1, false, 20_000, false),
        ChangeSectionsLayout {
            staged: ChangeSectionHeight::Fill,
            unstaged: ChangeSectionHeight::Fill,
        }
    );
}

#[test]
fn loading_section_keeps_space_while_empty_peer_stays_compact() {
    assert_eq!(
        change_sections_layout(0, true, 0, false),
        ChangeSectionsLayout {
            staged: ChangeSectionHeight::Fill,
            unstaged: ChangeSectionHeight::Compact,
        }
    );
}
