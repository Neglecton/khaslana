use super::*;

#[test]
fn theme_display_labels_cover_each_visual_state() {
    assert_eq!(theme_mode_id(ThemeMode::System), "system");
    assert_eq!(theme_mode_label(ThemeMode::Light), "浅色");
    assert_eq!(theme_variant_label(ThemeVariant::Dark), "深色主题");
}
