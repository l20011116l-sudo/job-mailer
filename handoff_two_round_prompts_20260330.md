# job-mailer：两轮提示词拆分 — 交接（2026-03-30）

> 供**新会话**快速接续：本文件记录已完成的实现、调试埋点、日志位置，以及**仍未达标**的用户反馈与待查项。  
> 路由见 `context-infrastructure/rules/WORKSPACE.md`（求职桌面 App：`../job-mailer/`）。

---

## 1. 已实现内容（代码层面）

### 1.1 目标

将 Settings 里原先混在一起的「指令块」拆成：

- **第一轮**：JD 对齐（独立 **system** 提示词，可覆盖内置 `JD_ALIGNMENT_SYSTEM_PROMPT`）。
- **第二轮**：邮件生成 — **system**（沿用应用内 `systemPrompt` / 默认 `DEFAULT_SYSTEM_PROMPT` 概念）+ **user 末尾硬性约束**（`email_user_suffix_prompt`，空则使用内置 `USER_GENERATION_TAIL` 类默认尾部）。

### 1.2 Rust（`src-tauri/src/lib.rs`）

- `StoredSettings` 增加/使用：`jd_alignment_system_prompt`、`email_user_suffix_prompt`（命名以源码为准）。
- `generate_jd_alignment_paragraph(..., system_prompt_override)`：第一轮可注入自定义 system。
- `generate_email(..., jd_alignment_system_prompt, email_user_suffix_prompt)`：第二轮接收两轮相关字符串；对过长自定义 prompt 有截断上限（如 `MAX_JD_ALIGNMENT_SYSTEM_PROMPT_CHARS`）。
- 仍保留：JD 对齐开关、主生成、**defluff** 第三调用（去套话等，非用户可编辑独立区块）。

### 1.3 前端（`src/App.tsx`）

- 设置页展示：**第一轮 JD 对齐 (system)**、**第二轮 邮件生成 (system)**、**第二轮 user 末尾硬性约束**。
- `invoke("generate_email", { ..., jdAlignmentSystemPrompt, emailUserSuffixPrompt })`（Tauri 前端 camelCase ↔ Rust snake_case，若运行时失败需核对字段名）。

### 1.4 自检

- 曾通过：`cargo check`、`npx tsc --noEmit`（以当前分支为准重新跑一遍更稳妥）。

---

## 2. 已做的 Debug / 观测

### 2.1 结构化日志（session `9dc719`）

- 运行时写入多路径 JSONL（见单次运行中的 `log_path_*`），例如：
  - `~/Library/Application Support/job-mailer/debug-9dc719.log`
  - 仓库内：`context-infrastructure/tmp/debug-9dc719.log`、`job-mailer/debug-9dc719.log`、`.cursor/debug-9dc719.log`
- **hypothesisId 含义（便于对照 `lib.rs`）**：
  - **H0**：`generate_email` 入口（JD 长、简历/策略长度等）。
  - **H1**：第一轮 `generate_jd_alignment_paragraph` 成功（alignment 字符数等）。
  - **H1 / after_alignment_step**：第一轮结束后进入第二轮前的状态。
  - **H2–H5**：`before_main_completion` — `system_chars`、`user_content_chars`、`uses_default_system_prompt`、`strategy_placeholder` 等（用于区分是否用了默认第二轮 system、user 体量）。
  - **H6**：主生成正文 **`output_fluff_heuristic_pre_defluff`**（套话启发式、`fluff_marker_hits`）。
  - **H7**：**defluff** 是否应用（`defluff_applied` / 错误时另有跳过类事件）。

### 2.2 日志中曾观测到的现象（示例）

- 同一会话多次 `invoke`：`uses_default_system_prompt` 在 true/false 间切换（对应用户在设置里清空或填写第二轮 system）。
- 某次生成：`fluff_marker_hits` 与 `markers: ["希望能在"]`，随后 defluff 管线参与（具体是否改善正文依赖人工读邮件）。

### 2.3 编译侧

- 曾出现 `strip_traditional_letter_closing` **dead_code** 警告（未使用函数），与功能正确性无直接关系，可后续删除或接入。

---

## 3. 未解决 / 用户反馈（当前仍「不及预期」）

### 3.1 主观质量

- 用户反馈：重启应用后效果**仍不及预期**（未单独开 bug 单，属**质量/风格**层面）。
- 用户给出的**示例正文片段**（金杜上海分所、医药并购及合规实习生、法大硕士等）整体偏**标准求职信/自我介绍**，偏正式、概括性强；若目标是个性化、更贴 JD 颗粒度或更短更狠的文体，需要**明确产品标准**后再改 prompt 或流程（例如是否弱化「您好」起手、是否强制引用 JD 原文关键词等）。

### 3.2 待核实（若复现再修）

- 若生成正文中出现 **`===`** 或与 UI 无关的**指令泄漏**，需对照当时 `before_main_completion` 的 user 拼接与模型输出单独复现（当前交接消息里用户粘贴可能在 `随信附上我的简历。` 之后混入了对本 agent 的说明，**以实际 App 导出为准**）。

### 3.3 产品/实现上的后续选项（未做）

- README / 应用内文案：对三个设置项的含义与推荐填法做简短说明。
- 稳定后：**精简或移除** `#region agent log` 类调试写入，避免磁盘与隐私噪音。
- **defluff** 仍为第三次模型调用；若用户希望完全可控，可考虑开关或合并进第二轮 prompt（权衡成本与质量）。

---

## 4. 下次新聊天建议起手式

1. 读 `context-infrastructure/rules/WORKSPACE.md` → 打开本文件与 `job-mailer/src-tauri/src/lib.rs`。
2. 让用户提供：**目标文体示例**（满意的一封）与**当前一封**对比；必要时贴 `settings.json` 中相关 prompt 字段（脱敏）或 `debug-*.log` 中 `before_main_completion` 一条。
3. 区分问题是 **第一轮 alignment 太薄**、**第二轮 system 约束不够**，还是 **defluff 改坏了**，再改提示词或管线。

---

## 5. 关键路径速查

| 内容 | 路径 |
|------|------|
| 桌面应用源码 | `job-mailer/` |
| 默认/内置 prompt 常量 | `job-mailer/src-tauri/src/lib.rs` |
| UI | `job-mailer/src/App.tsx` |
| 工作区路由规则 | `context-infrastructure/rules/WORKSPACE.md` |
| 调试日志（示例 session） | `job-mailer/debug-9dc719.log`、`context-infrastructure/tmp/debug-9dc719.log` |
