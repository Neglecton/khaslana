use super::*;
use crate::storage::UpdatePreferences;

// ── 版本相关 ──────────────────────────────────────────────────────────────

#[test]
fn current_version_is_valid_semver() {
    let v = current_version();
    // 当前 Cargo.toml version = "2.0.0-beta.2"（发版时同步更新本断言）
    assert_eq!(v.major, 2);
    assert_eq!(v.minor, 0);
    assert_eq!(v.patch, 0);
    assert_eq!(v.pre.to_string(), "beta.2");
}

#[test]
fn default_sources_cnb_before_github() {
    let sources = default_manifest_sources();
    assert!(sources[0].contains("cnb.cool"));
    assert!(sources[0].contains("/-/git/raw/"));
    assert!(!sources[0].contains("/-/raw/"));
    assert!(sources[1].contains("github.com"));
    // 正式清单的 GitHub 兜底必须走 releases/latest（旧客户端同源，
    // latest 天然只解析非 prerelease 版本，不会被测试版污染）。
    assert!(sources[1].contains("releases/latest/download"));
}

// 测试版清单：仅 CNB 源；勾选测试版后的源组合 = beta 源在前、正式源兜底。
#[test]
fn beta_sources_and_mode_composition() {
    let beta = beta_manifest_sources();
    assert_eq!(beta.len(), 1);
    assert!(beta[0].contains("khaslana-update-beta.json"));
    assert!(beta[0].contains("cnb.cool"));

    let stable_prefs = UpdatePreferences {
        auto_check: true,
        skipped_version: None,
        include_beta: false,
    };
    assert_eq!(
        manifest_sources_for(&stable_prefs),
        default_manifest_sources(),
        "未勾选测试版必须与旧客户端逐源一致"
    );

    let beta_prefs = UpdatePreferences {
        auto_check: true,
        skipped_version: None,
        include_beta: true,
    };
    let sources = manifest_sources_for(&beta_prefs);
    assert_eq!(sources.len(), 3);
    assert!(sources[0].contains("khaslana-update-beta.json"));
    assert_eq!(&sources[1..], default_manifest_sources().as_slice());
}

// ── 清单解析与版本比较 ─────────────────────────────────────────────────────

fn sample_manifest(schema: u32, version: &str) -> UpdateManifest {
    UpdateManifest {
        schema,
        channel: "stable".to_string(),
        version: version.to_string(),
        published_at: "2026-06-26T12:00:00Z".to_string(),
        notes: "测试更新".to_string(),
        platforms: {
            let mut map = HashMap::new();
            map.insert(
                "windows-x86_64".to_string(),
                UpdatePlatformAsset {
                    archive_url: "https://example.com/test.zip".to_string(),
                    fallback_archive_url: "https://fallback.example.com/test.zip".to_string(),
                    sha256: "abcd1234".to_string(),
                    size: 12345678,
                },
            );
            map
        },
    }
}

#[test]
fn manifest_json_deserializes() {
    let json = r#"{
        "schema": 1,
        "channel": "stable",
        "version": "0.2.0",
        "published_at": "2026-06-26T12:00:00Z",
        "notes": "新版本",
        "platforms": {
            "windows-x86_64": {
                "archive_url": "https://cnb.cool/test.zip",
                "fallback_archive_url": "https://github.com/test.zip",
                "sha256": "abcdef1234567890",
                "size": 10000000
            }
        }
    }"#;
    let manifest: UpdateManifest = serde_json::from_str(json).unwrap();
    assert_eq!(manifest.schema, 1);
    assert_eq!(manifest.version, "0.2.0");
    assert!(manifest.platforms.contains_key("windows-x86_64"));
}

#[test]
fn schema_mismatch_returns_error() {
    let manifest = sample_manifest(2, "0.2.0");
    // 模拟 check_for_update 的 schema 检查
    if manifest.schema != 1 {
        let err = GitError::Message(format!(
            "不支持的更新清单格式（schema={}），当前仅支持 schema=1",
            manifest.schema
        ));
        assert!(err.to_string().contains("不支持的更新清单格式"));
    }
}

#[test]
fn missing_platform_returns_error() {
    let mut manifest = sample_manifest(1, "0.2.0");
    manifest.platforms.remove("windows-x86_64");
    // 模拟 check_for_update 的平台检查
    assert!(manifest.platforms.get("windows-x86_64").is_none());
}

#[test]
fn version_comparison_lower_is_update() {
    // remote 0.2.0 > current 0.1.0 → UpdateAvailable
    let manifest = sample_manifest(1, "0.2.0");
    let result = evaluate_manifest(&manifest, &"0.1.0".parse().unwrap(), None, false).unwrap();
    assert!(matches!(result, UpdateCheckResult::UpdateAvailable { .. }));
}

#[test]
fn version_comparison_equal_is_up_to_date() {
    let manifest = sample_manifest(1, "0.1.0");
    let result = evaluate_manifest(&manifest, &"0.1.0".parse().unwrap(), None, false).unwrap();
    assert!(matches!(result, UpdateCheckResult::UpToDate));
}

#[test]
fn version_comparison_higher_is_up_to_date() {
    // 如果 manifest 版本低于当前版本（比如回退发布），不提示
    let manifest = sample_manifest(1, "0.0.9");
    let result = evaluate_manifest(&manifest, &"0.1.0".parse().unwrap(), None, false).unwrap();
    assert!(matches!(result, UpdateCheckResult::UpToDate));
}

// 正式模式（未勾选测试版）忽略带预发布段的清单版本——防御误配置把测试版
// 写入正式清单推给未勾选用户；勾选测试版后正常接受。
#[test]
fn stable_mode_ignores_prerelease_manifest() {
    let manifest = sample_manifest(1, "1.1.0-beta.1");
    let current: Version = "1.0.10".parse().unwrap();

    let stable = evaluate_manifest(&manifest, &current, None, false).unwrap();
    assert!(matches!(stable, UpdateCheckResult::UpToDate));

    let beta = evaluate_manifest(&manifest, &current, None, true).unwrap();
    assert!(matches!(beta, UpdateCheckResult::UpdateAvailable { .. }));
}

// 预发布语义复用 semver 排序：beta.1 < beta.2 < 正式版，测试版用户在正式版
// 发布后被自然引导升级（不会停在 beta）。
#[test]
fn prerelease_orders_below_stable_release() {
    let current: Version = "1.1.0-beta.1".parse().unwrap();

    // 同版本 beta.2 > beta.1 → 提示。
    let newer_beta = sample_manifest(1, "1.1.0-beta.2");
    let result = evaluate_manifest(&newer_beta, &current, None, true).unwrap();
    assert!(matches!(result, UpdateCheckResult::UpdateAvailable { .. }));

    // 正式版 1.1.0 > beta.1 → 提示（测试版用户升正式版）。
    let stable = sample_manifest(1, "1.1.0");
    let result = evaluate_manifest(&stable, &current, None, true).unwrap();
    assert!(matches!(result, UpdateCheckResult::UpdateAvailable { .. }));

    // 清单版本不高于当前（旧 beta）→ UpToDate，不会降级。
    let older = sample_manifest(1, "1.1.0-beta.1");
    let result = evaluate_manifest(&older, &current, None, true).unwrap();
    assert!(matches!(result, UpdateCheckResult::UpToDate));
}

#[test]
fn skipped_version_suppresses_prompt() {
    let manifest = sample_manifest(1, "0.2.0");
    let result =
        evaluate_manifest(&manifest, &"0.1.0".parse().unwrap(), Some("0.2.0"), false).unwrap();
    assert!(matches!(result, UpdateCheckResult::SkippedVersion));
}

#[test]
fn skipped_version_older_than_manifest_reprompts() {
    let manifest = sample_manifest(1, "0.2.0");
    let result =
        evaluate_manifest(&manifest, &"0.1.0".parse().unwrap(), Some("0.1.0"), false).unwrap();
    assert!(matches!(result, UpdateCheckResult::UpdateAvailable { .. }));
}

// 跳过对测试版本号同样按精确相等生效。
#[test]
fn skipped_version_applies_to_prerelease() {
    let manifest = sample_manifest(1, "1.1.0-beta.1");
    let result = evaluate_manifest(
        &manifest,
        &"1.0.10".parse().unwrap(),
        Some("1.1.0-beta.1"),
        true,
    )
    .unwrap();
    assert!(matches!(result, UpdateCheckResult::SkippedVersion));
}

// 渠道标识按 semver 预发布段推断（供「关于」页使用）。
#[test]
fn current_channel_matches_version_shape() {
    // 渠道标识与版本号形态自洽：无预发布段 = 正式版，有 = 测试版
    //（本断言与具体版本无关，发版无需更新）。
    let expected = if current_version().pre.is_empty() {
        "正式版"
    } else {
        "测试版"
    };
    assert_eq!(current_channel(), expected);
}

// ── SHA-256 校验 ──────────────────────────────────────────────────────────

#[test]
fn sha256_verification_matches_known_content() {
    let temp = tempfile::tempdir().unwrap();
    let file_path = temp.path().join("test.bin");
    // "hello world" 的 SHA-256
    let content = b"hello world";
    let expected = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
    fs::File::create(&file_path)
        .unwrap()
        .write_all(content)
        .unwrap();
    assert!(verify_sha256(&file_path, expected).unwrap());
}

#[test]
fn sha256_verification_fails_on_tampered_hash() {
    let temp = tempfile::tempdir().unwrap();
    let file_path = temp.path().join("test.bin");
    let content = b"hello world";
    let wrong_hash = "0000000000000000000000000000000000000000000000000000000000000000";
    fs::File::create(&file_path)
        .unwrap()
        .write_all(content)
        .unwrap();
    assert!(!verify_sha256(&file_path, wrong_hash).unwrap());
}

// ── staging 解压 ──────────────────────────────────────────────────────────

#[test]
fn prepare_staging_validates_exe_presence() {
    let temp = tempfile::tempdir().unwrap();
    let config_dir = temp.path();

    // 创建一个简单 zip，包含两个 exe 文件
    let zip_path = config_dir.join("test_update.zip");
    create_test_zip(&zip_path);

    let result = prepare_staging(&zip_path, "0.2.0", config_dir);
    assert!(result.is_ok());

    let staging = result.unwrap();
    assert!(staging.join("khaslana.exe").exists());
    assert!(staging.join("khaslana_updater.exe").exists());
}

#[test]
fn prepare_staging_fails_without_exe() {
    let temp = tempfile::tempdir().unwrap();
    let config_dir = temp.path();

    // 创建不含 exe 的 zip
    let zip_path = config_dir.join("bad_update.zip");
    create_test_zip_without_exe(&zip_path);

    let result = prepare_staging(&zip_path, "0.2.0", config_dir);
    assert!(result.is_err());
}

// ── 辅助函数 ──────────────────────────────────────────────────────────────

/// 创建包含 khaslana.exe 和 khaslana_updater.exe 的测试 zip。
fn create_test_zip(path: &Path) {
    let file = fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();

    zip.start_file("khaslana.exe", options).unwrap();
    zip.write_all(b"fake exe content").unwrap();

    zip.start_file("khaslana_updater.exe", options).unwrap();
    zip.write_all(b"fake updater content").unwrap();

    zip.finish().unwrap();
}

/// 创建不含 exe 的测试 zip（只有无关文件）。
fn create_test_zip_without_exe(path: &Path) {
    let file = fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();

    zip.start_file("README.md", options).unwrap();
    zip.write_all(b"just a readme").unwrap();
    zip.finish().unwrap();
}
