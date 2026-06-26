use super::*;

#[cfg(target_os = "windows")]
#[test]
fn windows_native_path_converts_forward_slashes_for_explorer() {
    assert_eq!(
        windows_native_path(Path::new(r"D:/devProjects/workplace/khaslana")),
        r"D:\devProjects\workplace\khaslana"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn windows_native_path_strips_extended_length_prefix_for_explorer() {
    assert_eq!(
        windows_native_path(Path::new(r"\\?\D:\devProjects\workplace\khaslana")),
        r"D:\devProjects\workplace\khaslana"
    );
}

#[cfg(target_os = "windows")]
#[test]
fn windows_explorer_directory_arg_uses_folder_mode() {
    assert_eq!(
        windows_explorer_directory_arg(Path::new(r"D:/devProjects/workplace/khaslana")),
        r"/e,D:\devProjects\workplace\khaslana"
    );
}
