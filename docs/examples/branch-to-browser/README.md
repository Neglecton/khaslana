# 新建分支 → 合并分支 → Skill 填写浏览器

样板使用现有 v2 Git 动作与 `skill.run`。分支名称通过 `${target}` 直接进入 Skill 任务，不使用系统剪贴板。目标是 [Selenium 的公开表单](https://www.selenium.dev/selenium/web/web-form.html) 的 **Text input**，填写后读取快照复核，不提交表单。

## 导入

1. 在 AI 设置中启用并配置支持工具调用的模型。
2. 在 MCP 中添加服务，服务 ID 为 `chrome-devtools`，命令 `npx`，参数每行一个：`-y`、`chrome-devtools-mcp@latest`。测试成功后保存。需要本机 Node.js/npm 和 Chrome；服务 ID 是设置中保存的名称，不是 npm 包名。已有服务无需重建；若名称不同，修改模板 `tools` 中的三处名称，使其与设置完全一致。
3. 在 Skill 页导入本目录下的 `workflow-skills/branch-to-browser` 文件夹。
4. 在工作流页选择本目录的 `create-merge-fill.json5`，或将文件放入工作流页面“打开目录”所显示的模板目录后刷新。
5. 填写创建起点、新分支名和要合并的分支，在步骤预览及运行确认中检查后运行。

普通添加流程默认启用工具；本样板只申请 `new_page`、`take_snapshot`、`fill` 三个工具。参数定义见 [Chrome DevTools MCP 官方参考](https://github.com/ChromeDevTools/chrome-devtools-mcp/blob/main/docs/tool-reference.md)，运行时以服务实际返回的定义为准。

## 执行结果

先创建并切换到新分支，再将 `source` 合并进去，核对当前分支后执行 Skill。浏览器会打开一个新标签页，Text input 显示新分支名称。Skill 最终说明保存为工作流变量 `browserResult`。

工作区必须干净，新分支不能已存在，起点和合并来源必须是现有引用。合并发生冲突时停止，浏览器步骤不会执行；Git 已完成的创建或合并不会因为浏览器失败而回滚。此样板不推送远端。

可先在测试仓库演练。不要在同名新分支已经创建后原样重跑整条流程；应换一个新分支名，或单独排查最后的浏览器步骤。
