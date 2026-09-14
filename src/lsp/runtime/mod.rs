//! 运行环境检测子模块。Java 候选发现与版本探测见 [`java`]。

pub mod java;

pub use java::{
    JdkCandidate, JdkDetectionError, JdkProbe, JdkSource, discover_java_homes, default_probe,
    layout_complete, normalize_arch, parse_java_major, parse_show_settings_output,
    probe_java_runtime,
};
