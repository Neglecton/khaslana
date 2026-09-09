//! Java 静态事实与项目组织模型（DEV-04）。
//!
//! 本层只提取声明、类型、词法作用域与模块/源集事实；调用绑定和动态分派属于
//! DEV-05，不在这里根据短名猜测目标。

pub mod extract;
pub mod project;
pub mod types;

pub use project::{JavaProjectDiagnostic, JavaProjectModel, JavaSourceScope};
pub use types::{
    JavaFileFacts, JavaImportFact, JavaParameterFact, JavaSymbolFact, JavaVariableFact,
};
