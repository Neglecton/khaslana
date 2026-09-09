use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JavaSourceScope {
    pub module_key: String,
    pub source_set: String,
    pub source_root: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JavaProjectDiagnostic {
    pub rel_path: String,
    pub code: String,
    pub detail: String,
}

/// Maven/Gradle 的有限静态项目模型。它不执行构建脚本，也不下载父 POM 或依赖。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JavaProjectModel {
    module_roots: BTreeMap<String, String>,
    pub diagnostics: Vec<JavaProjectDiagnostic>,
}

impl JavaProjectModel {
    /// 输入内容来自管线已经读取并计算 hash 的同一批字节，避免项目模型二次读盘。
    pub fn from_sources<'a>(sources: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let sources: Vec<_> = sources.into_iter().collect();
        let mut model = Self::default();
        let available: BTreeSet<&str> = sources.iter().map(|(path, _)| *path).collect();

        for (rel_path, text) in &sources {
            let lower = rel_path.to_ascii_lowercase();
            if lower.ends_with("pom.xml") {
                let root = parent_dir(rel_path);
                for module in xml_elements(text, "module") {
                    let module = normalize_rel(&module);
                    if module.is_empty() || module.contains("${") {
                        model.diagnostics.push(JavaProjectDiagnostic {
                            rel_path: (*rel_path).to_string(),
                            code: "java_dynamic_module".into(),
                            detail: format!("无法静态解析 Maven module：{module}"),
                        });
                        continue;
                    }
                    let module_root = join_rel(root, &module);
                    model
                        .module_roots
                        .insert(module_root.clone(), module_root.clone());
                    let pom = join_rel(&module_root, "pom.xml");
                    if !available.contains(pom.as_str()) {
                        model.diagnostics.push(JavaProjectDiagnostic {
                            rel_path: (*rel_path).to_string(),
                            code: "java_missing_module_descriptor".into(),
                            detail: format!("Maven module 缺少可见 pom.xml：{pom}"),
                        });
                    }
                }
            } else if lower.ends_with("settings.gradle") || lower.ends_with("settings.gradle.kts") {
                let root = parent_dir(rel_path);
                for module in gradle_includes(text) {
                    let module_path = module.trim_start_matches(':').replace(':', "/");
                    if module_path.is_empty() {
                        continue;
                    }
                    let module_root = join_rel(root, &module_path);
                    model.module_roots.insert(module_root.clone(), module_root);
                }
                for (module, project_dir) in gradle_project_dirs(text) {
                    let module_key = module.trim_start_matches(':').replace(':', "/");
                    model
                        .module_roots
                        .retain(|_, existing_key| existing_key != &module_key);
                    model
                        .module_roots
                        .insert(join_rel(root, &normalize_rel(&project_dir)), module_key);
                }
                if text.lines().any(|line| {
                    let line = line.trim();
                    is_gradle_include_line(line) && !line.contains(['\'', '"'])
                }) {
                    model.diagnostics.push(JavaProjectDiagnostic {
                        rel_path: (*rel_path).to_string(),
                        code: "java_dynamic_module".into(),
                        detail: "Gradle include 不是静态字符串，模块可见性未知".into(),
                    });
                }
                if text.lines().any(|line| {
                    let line = line.trim();
                    line.contains("project(")
                        && line.contains("projectDir")
                        && first_quoted(&line[line.find("projectDir").unwrap()..]).is_none()
                }) {
                    model.diagnostics.push(JavaProjectDiagnostic {
                        rel_path: (*rel_path).to_string(),
                        code: "java_dynamic_project_dir".into(),
                        detail: "Gradle projectDir 不是静态字符串，模块根未知".into(),
                    });
                }
            }
        }
        model
    }

    pub fn scope_for(&self, rel_path: &str) -> JavaSourceScope {
        let rel_path = normalize_rel(rel_path);
        let conventions = [
            ("src/main/java", "main"),
            ("src/test/java", "test"),
            ("src/main/kotlin", "main"),
            ("src/test/kotlin", "test"),
        ];
        for (root_suffix, source_set) in conventions {
            if rel_path == root_suffix || rel_path.starts_with(&format!("{root_suffix}/")) {
                return JavaSourceScope {
                    module_key: String::new(),
                    source_set: source_set.into(),
                    source_root: root_suffix.into(),
                };
            }
            let marker = format!("/{root_suffix}/");
            if let Some(index) = rel_path.find(&marker) {
                let module_root = &rel_path[..index];
                return JavaSourceScope {
                    module_key: self
                        .module_roots
                        .get(module_root)
                        .cloned()
                        .unwrap_or_else(|| {
                            module_root
                                .rsplit('/')
                                .next()
                                .unwrap_or(module_root)
                                .to_string()
                        }),
                    source_set: source_set.into(),
                    source_root: format!("{module_root}/{root_suffix}"),
                };
            }
        }

        let module = self
            .module_roots
            .iter()
            .filter(|(root, _)| rel_path == **root || rel_path.starts_with(&format!("{root}/")))
            .max_by_key(|(root, _)| root.len());
        JavaSourceScope {
            module_key: module.map(|(_, key)| key.clone()).unwrap_or_default(),
            source_set: "main".into(),
            source_root: module.map(|(root, _)| root.clone()).unwrap_or_default(),
        }
    }

    pub fn module_roots(&self) -> &BTreeMap<String, String> {
        &self.module_roots
    }
}

fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("")
}

fn join_rel(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_string()
    } else if child.is_empty() {
        parent.to_string()
    } else {
        format!("{parent}/{child}")
    }
}

fn normalize_rel(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let mut out = Vec::new();
    for part in normalized.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            _ => out.push(part),
        }
    }
    out.join("/")
}

fn xml_elements(text: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut values = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(&open) {
        rest = &rest[start + open.len()..];
        let Some(end) = rest.find(&close) else { break };
        values.push(rest[..end].trim().to_string());
        rest = &rest[end + close.len()..];
    }
    values
}

fn gradle_includes(text: &str) -> Vec<String> {
    let mut modules = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if !is_gradle_include_line(line) {
            continue;
        }
        let mut rest = &line["include".len()..];
        while let Some(start) = rest.find(['\'', '"']) {
            let quote = rest[start..].chars().next().unwrap();
            rest = &rest[start + quote.len_utf8()..];
            let Some(end) = rest.find(quote) else { break };
            modules.push(rest[..end].to_string());
            rest = &rest[end + quote.len_utf8()..];
        }
    }
    modules
}

fn is_gradle_include_line(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("include") else {
        return false;
    };
    rest.is_empty()
        || rest
            .chars()
            .next()
            .is_some_and(|ch| ch.is_whitespace() || ch == '(')
}

fn gradle_project_dirs(text: &str) -> Vec<(String, String)> {
    let mut values = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(project_start) = line.find("project(") else {
            continue;
        };
        let project_args = &line[project_start + "project(".len()..];
        let Some(module) = first_quoted(project_args) else {
            continue;
        };
        let Some(project_dir) = line.find("projectDir") else {
            continue;
        };
        let Some(path) = first_quoted(&line[project_dir + "projectDir".len()..]) else {
            continue;
        };
        values.push((module, path));
    }
    values
}

fn first_quoted(text: &str) -> Option<String> {
    let start = text.find(['\'', '"'])?;
    let quote = text[start..].chars().next()?;
    let rest = &text[start + quote.len_utf8()..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maven_modules_and_source_sets_are_isolated() {
        let model = JavaProjectModel::from_sources([
            (
                "pom.xml",
                "<project><modules><module>app-core</module><module>app-web</module></modules></project>",
            ),
            ("app-core/pom.xml", "<project/>"),
            ("app-web/pom.xml", "<project/>"),
        ]);
        assert_eq!(
            model.scope_for("app-core/src/main/java/com/example/Named.java"),
            JavaSourceScope {
                module_key: "app-core".into(),
                source_set: "main".into(),
                source_root: "app-core/src/main/java".into(),
            }
        );
        assert_eq!(
            model
                .scope_for("app-core/src/test/java/com/example/NamedTest.java")
                .source_set,
            "test"
        );
        assert_eq!(
            model
                .scope_for("app-web/src/main/java/com/example/Named.java")
                .module_key,
            "app-web"
        );
    }

    #[test]
    fn gradle_static_include_and_project_dir_are_respected() {
        let model = JavaProjectModel::from_sources([(
            "settings.gradle",
            "include ':api', ':core:impl'\nproject(':api').projectDir = file('modules/public-api')",
        )]);
        assert_eq!(
            model
                .scope_for("modules/public-api/src/main/java/demo/Api.java")
                .module_key,
            "api"
        );
        assert_eq!(
            model
                .scope_for("core/impl/src/test/java/demo/ImplTest.java")
                .module_key,
            "core/impl"
        );
        assert!(!model.module_roots().contains_key("api"));
    }

    #[test]
    fn gradle_dynamic_roots_are_diagnostic_and_include_build_is_ignored() {
        let model = JavaProjectModel::from_sources([(
            "settings.gradle",
            "include dynamicModule\nincludeBuild '../shared'\nproject(':api').projectDir = computedDir",
        )]);
        assert!(model.module_roots().is_empty());
        assert!(
            model
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "java_dynamic_module")
        );
        assert!(
            model
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "java_dynamic_project_dir")
        );
    }
}
