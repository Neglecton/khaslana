use serde::{Deserialize, Serialize};

/// Java import 的语法事实。`path` 保留完整导入路径；静态成员与通配信息拆开，
/// 后续解析不需要再猜原始文本形态。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JavaImportFact {
    pub path: String,
    pub is_static: bool,
    pub is_wildcard: bool,
    pub start_byte: u32,
    pub end_byte: u32,
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JavaParameterFact {
    pub name: String,
    /// 规范化后的类型。能在本文件上下文中证明时使用 FQN；否则以 `?` 开头。
    pub type_name: String,
    pub raw_type: String,
    pub varargs: bool,
    pub start_byte: u32,
    pub end_byte: u32,
}

/// 参数/局部变量等词法作用域事实。字段声明作为独立 JavaSymbolFact 保存。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JavaVariableFact {
    pub name: String,
    pub type_name: String,
    pub raw_type: String,
    pub kind: String,
    pub owner_chain: Vec<String>,
    pub scope_start_byte: u32,
    pub scope_end_byte: u32,
    pub start_byte: u32,
    pub end_byte: u32,
}

/// 可独立持久化的 Java 声明。`owner_chain` 不含声明自身；嵌套/局部类型的
/// owner 链会包含外层类型（局部类型还包含方法名）。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JavaSymbolFact {
    pub kind: String,
    pub name: String,
    pub owner_chain: Vec<String>,
    /// 方法/构造器为 `(<参数类型,...>)`，其他声明为空。
    pub signature: String,
    pub parameters: Vec<JavaParameterFact>,
    pub return_type: Option<String>,
    pub raw_return_type: Option<String>,
    pub declared_type: Option<String>,
    pub raw_declared_type: Option<String>,
    pub modifiers: Vec<String>,
    pub annotations: Vec<String>,
    pub javadoc: Option<String>,
    pub extends: Vec<String>,
    pub implements: Vec<String>,
    pub permits: Vec<String>,
    pub start_byte: u32,
    pub end_byte: u32,
    pub start_line: u32,
    pub end_line: u32,
    /// `stable` 或 `local`。局部/匿名范围不能作为跨移动稳定身份。
    pub stability: String,
}

impl JavaSymbolFact {
    pub fn identity_name(&self) -> &str {
        if self.kind == "constructor" {
            "<init>"
        } else {
            &self.name
        }
    }

    pub fn is_callable(&self) -> bool {
        matches!(self.kind.as_str(), "method" | "constructor")
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JavaFileFacts {
    pub package: String,
    pub imports: Vec<JavaImportFact>,
    pub symbols: Vec<JavaSymbolFact>,
    pub variables: Vec<JavaVariableFact>,
}
