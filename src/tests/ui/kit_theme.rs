use super::*;

/// 深浅变体必须映射到对应的 Kit mode：`Theme::from(&ThemeColor)` 恒写
/// Light，桥接若漏了这层映射，深色主题下 Kit 输入组等组件的 is_dark()
/// 分支会走错（审查 R2）。
#[test]
fn kit_theme_mode_follows_variant() {
    assert_eq!(kit_theme_mode(ThemeVariant::Light), ThemeMode::Light);
    assert!(!kit_theme_mode(ThemeVariant::Light).is_dark());
    assert_eq!(kit_theme_mode(ThemeVariant::Dark), ThemeMode::Dark);
    assert!(kit_theme_mode(ThemeVariant::Dark).is_dark());
}
