# 法学本科生短期实习定制化海投工具

（内部工程名：**Job Mailer**）

macOS 本地桌面应用（Tauri + React）：根据岗位描述与可选 Markdown 策略资产，调用 **OpenAI 兼容** 的 `/chat/completions` 生成求职邮件。默认**两阶段**：先按 JD 生成第三方备忘录（业务白话 / JD 事实 / 岗位侧要求），再合并「内置准备模块」指令写全文；经人工审核后 **SMTP** 发送，可选 PDF 简历附件。默认 system prompt 见 `src-tauri/src/lib.rs` 中 `DEFAULT_SYSTEM_PROMPT`。

数据与 API Key、SMTP 密码仅保存在本机配置目录（通常为 `~/Library/Application Support/job-mailer/`）下的 `settings.json`，不上传至作者服务器。生成历史在同级 `generation_history.json`，最多 50 条，便于同一策略多岗位复用。

## 环境要求

- macOS（构建目标为 `dmg`）
- [Rust](https://rustup.rs/)（stable）
- Node.js 20+

## 开发运行

```bash
cd job-mailer
npm install
npm run tauri:dev
```

若终端里未把 `~/.cargo/bin` 加入 `PATH`，会出现 `cargo metadata ... No such file or directory`。请优先用上面的 `tauri:dev`（脚本会临时带上 Cargo 路径），或在 shell 中执行 `source ~/.zshrc` 后改用 `npm run tauri dev`。

## 发布构建

```bash
npm run tauri build
```

产物在 `src-tauri/target/release/bundle/`。

## 配置说明

- **API**：填写 Base URL（无末尾斜杠）、Key、模型 ID。需厂商提供 Chat Completions 兼容端点。
- **SMTP**：如 Gmail，请使用「应用专用密码」；端口常用 587（STARTTLS）。
- **发件人**：需为可被 SMTP 认证的地址，格式可为 `姓名 <email@domain.com>`。

## License

MIT（若你修改发布，请保留许可证与来源说明。）
