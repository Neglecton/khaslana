---
name: edge-web-form
description: 在 Selenium 公开演示表单中读取页面并填写文本输入框。
---

只访问 https://www.selenium.dev/selenium/web/web-form.html 。
先调用 browser_navigate，再调用 browser_snapshot。只有页面 URL、标题 Web form 和 Text input 均匹配时，才可对 `input[name="my-text"]` 使用 browser_type，且必须设置 `submit: false`。
填写后再次读取 browser_snapshot，确认文本已出现在快照中。
如果页面不匹配、元素定位失败、工具超时或结果不确定，立即停止并在最终结果中说明。
不得点击 Submit，也不得调用任何提交、保存、上传或发送工具。
