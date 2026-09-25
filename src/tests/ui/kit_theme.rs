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

/// 选区底色是 accent 里唯一带透明度（`0xRRGGBBAA`）的字段，桥接必须用
/// `rgba()` 解析：`rgb()` 只认低 24 位，会把 alpha 字节当成蓝色通道，
/// 把 `0x16A34A33` 截成 `0xA34A33` 暗红棕色块（实测在输入框全选文字时复现）。
#[test]
fn accent_selection_alpha_survives_rgba_parsing() {
    for (name, palette) in ui_theme::ACCENT_PRESETS {
        for selection in [palette.selection.0, palette.selection.1] {
            let alpha = selection & 0xFF;
            assert_ne!(alpha, 0, "{name} 的选区色必须带透明度");

            let via_rgba = rgba(selection);
            assert_eq!(
                via_rgba.a,
                alpha as f32 / 255.0,
                "{name} 的选区色经 rgba() 解析后透明度必须保留"
            );

            // rgb() 会把 alpha 字节吃进蓝色通道，同时把红色顶掉——两者都必须错，
            // 否则说明有人把 selection 改回 rgb() 而测试没发现。
            let via_rgb = rgb(selection);
            assert_ne!(via_rgb.r, via_rgba.r, "{name} 的选区色不能经 rgb() 解析");
            assert_ne!(via_rgb.b, via_rgba.b, "{name} 的选区色不能经 rgb() 解析");
        }
    }
}
