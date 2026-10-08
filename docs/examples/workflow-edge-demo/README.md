# Edge 工作流验收样板

本目录用于验证工作流的 MCP、JavaScript 与 AI Skill 链路。两个模板只读取和填写 [Selenium 公开演示表单](https://www.selenium.dev/selenium/web/web-form.html)，填写后复核，不点击 Submit。

## 安装与运行

1. 在客户端工作流页打开模板目录，将 `edge-web-form.json5` 和 `edge-web-form-skill.json5` 复制进去，刷新后加载。
2. 两个模板直接使用内置 `browser.edge`，无需添加自定义 MCP 服务。本机需要安装 Microsoft Edge；缺少运行组件时，在客户端点击“下载并启用”。
3. 先运行「Edge 公开表单读取与填写」，核对授权弹窗中的导航、快照、填写三个工具，再授权本次运行。日志应显示完成，页面的 Text input 应显示输入内容。
4. 验收 AI 版本时，在设置中心启用并配置 AI，导入 `workflow-skills/edge-web-form` 文件夹，再运行「AI Skill：Edge 公开表单」。日志应显示 AI 决策、工具调用和最终结果。

`workflow-mcp.json5` 是旧版 `edgeDemo + npx.cmd` 配置，保留用于兼容性回归测试。新用户无需复制它；客户端只在内存中兼容这一精确配置，不改写用户文件。

两个模板默认允许存在未提交改动，因为它们不执行 Git 写入。只授权当前模板所声明的三个工具；页面不匹配、填写失败或复核失败时应停止。AI 样板使用当前配置的供应商，运行会产生模型请求。

## 可重复执行的专项测试

确定性 JS/Edge 链路：

```powershell
cargo test --lib edge_demo_reads_and_fills_real_public_page -- --ignored --nocapture
```

AI Skill 链路按上面的客户端运行步骤手动验收。自动测试保留模板解析、配置与授权准备检查，不执行 Skill；调用 Skill 执行器的模拟测试和真实模型专项测试均已移除。

JS/Edge 专项测试通过不代表客户端窗口的授权、焦点、取消或布局验收通过。
