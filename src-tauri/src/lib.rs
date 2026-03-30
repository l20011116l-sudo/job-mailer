use lettre::message::header::ContentType;
use lettre::message::{Attachment, Mailbox, Message, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{SmtpTransport, Transport};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

/// 主生成：**短 system**，避免长禁令被模型「浅读」；细则见 `USER_GENERATION_TAIL`（置于 user 消息**末尾**，利用近因优先）。
const DEFAULT_SYSTEM_PROMPT: &str = r#"你是求职邮件助手。**只输出一个 JSON 对象**，不要 Markdown 代码围栏，不要解释。JSON 有且仅有两个字符串字段：subject（邮件标题）、body（邮件正文，正文内可用 \n 换行）。

正文第一行单独为称呼；无具体联系人时写「您好，」。HR 以简历为主；不得编造未在材料中出现的实习、项目或成果。

**user 消息最底部的【硬性约束（最后阅读）】与本文同级，冲突时以硬性约束为准。**"#;

/// 拼在 user 消息**最后**，使禁令与结构说明离输出最近。
const USER_GENERATION_TAIL: &str = r#"

————————
【硬性约束（最后阅读｜优先级最高）】

篇幅：正文约 **400～500 字（汉字）**；勿写成极短或上千字长信。

须体现：（1）读过 JD、能复述岗位侧关键事实；（2）有可核对经历则展开一条，否则诚实指向简历附件，不虚构。

【内置准备模块（勿单列小标题）】须融入首段与第二段，不得用「一、二、三」或邮件里再套「【】」小标题。若上文含「岗位侧事实备忘录」，优先消化其中「业务白话」「JD事实」「岗位侧要求」，改写成第一人称（勿逐句照抄标签）。无策略、备忘录薄时仍落实：

（1）岗位白话 2～4 句：轻量说明该岗位在 JD 用语下大致解决什么问题；须有 JD/备忘录依据，禁止编造机构名、案例、数据。

（2）可核对事实链：团队/条线、业务动作、客户或监管场景、实习生交付/技能关键词——名词与短句优先。

（3）投递与岗位要求 1～2 句：学历、语言、出勤与 JD 明文要求对应；无直接经历则写「具体经历与证明材料见简历附件」。

【反套话（硬性）】禁止主要用态度句凑篇幅。禁用示例（含同义改写）：深感兴趣、我了解到、我关注贵团队、希望有机会、期待您的审阅、浓厚兴趣、学习热情、乐于主动学习、严谨负责、对…充满热情、契合/匹配/高度一致、持续关注、前沿领域、抱有求知欲、希望能在、希望贵团队、致力于 等。**改用**：JD 名词 + 短事实 + 可核对信息。

【JD 调研感】首段建议 5～8 句、约 180～280 字：前半写（1）（2）的自然邮件语；后半写个人与（3）；禁止用「契合」「一致」「匹配」代替事实。

【策略资产】有可核对条目则展开至少 1 条（名称/关键词+角色或结果）；占位或极少则直指简历，禁止用「关注」「热情」填字。

【Show, don't tell】名词与短事实链优先；少形容词；不自夸优秀/顶尖。

【称呼】首行称呼单独成行；禁止首行以「我了解到」「我是」或裸「我」起笔。

【结构】① JD 与业务白话、事实链 → ② 个人与策略/简历 → ③ 实习时间、到岗与附件。勿多处重复「期待审阅」。

【结尾与套话（硬性）】正文**不要**使用「此致」「敬礼」或类似书信结尾。收束时用一句即可，例如「随信附上简历 PDF」或「详见附件简历」。少用「期待」「希望能」「进一步沟通」「不断提升」「形成初步认知」「立志从事」「高度的责任心」等态度句堆叠；能用一句说清出勤与附件就不要拆成多句抒情。

若上文有个人简介：subject 从中取姓名、院校、学历等组标题，勿编造未出现信息。"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub system_prompt: String,
}

/// Gmail 587 等为 STARTTLS；465 为 SMTPS（隐式 TLS）。用错组合会长时间卡住或握手失败。
const SMTP_IO_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub from_email: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedEmail {
    pub subject: String,
    pub body: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMsg,
}

#[derive(Debug, Deserialize)]
struct ChatMsg {
    content: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct GenJson {
    subject: String,
    body: String,
}

fn app_data_dir() -> Result<PathBuf, String> {
    let dir = dirs::config_dir()
        .ok_or_else(|| "无法解析本机配置目录".to_string())?
        .join("job-mailer");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

fn app_settings_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("settings.json"))
}

fn history_path() -> Result<PathBuf, String> {
    Ok(app_data_dir()?.join("generation_history.json"))
}

const MAX_JD_CHARS: usize = 100_000;
const MAX_STRATEGY_CHARS: usize = 300_000;
/// 个人简介上限：仅参与生成请求，避免用户粘贴全文简历导致 token 浪费
const MAX_RESUME_PROFILE_CHARS: usize = 8_000;
const MAX_HISTORY_ENTRIES: usize = 50;

/// 置于 user 消息开头；细则在文末 `USER_GENERATION_TAIL`
const USER_TASK_PREAMBLE: &str = "【执行说明】首行称呼。第一段写清岗位白话 + 可核对事实（见上文备忘录与 JD），再过渡到个人；策略区若为占位不得虚构，应指向简历。**全文约 400～500 字**。细则以文末「硬性约束」为准。\n\n";

/// 模型未生成称呼时，由程序补全，保证各版本 prompt 下行为一致
const DEFAULT_SALUTATION_PREFIX: &str = "您好，\n\n";

const GENERATION_TEMPERATURE: f32 = 0.38;
const DEFLUFF_TEMPERATURE: f32 = 0.22;

/// 与 H6 启发式一致；用于可选二次「去套话」改写
const FLUFF_MARKERS: &[&str] = &[
    "求知欲",
    "持续关注",
    "前沿领域",
    "前沿问题",
    "抱有",
    "希望能在",
    "对……抱有",
];

const DEFLUFF_SYSTEM_PROMPT: &str = r#"你是严格编辑。只输出一个 JSON 对象，不要 Markdown 代码围栏。JSON 有且仅有字段：subject（字符串）、body（字符串）。

任务：在**保留 JD 可核对事实、学历、院校、语言与出勤安排**的前提下压缩废话。必须删除或改写：
· 书信套语：「此致」「敬礼」及类似结尾；
· 态度堆叠：「期待」「希望能」「期待能有机会」「进一步沟通」「立志从事」「高度的责任心」「主动进行…思考」「形成初步认知」「注重培养」「不断提升专业能力」等（除非直接复述 JD 原文要求）；
· 重复抒情：同一意思说多遍则合并为一句短事实。

结尾只保留**一句**附件/简历提示即可。用名词与短句，不要新增初稿中不存在的实习或项目。"#;

/// 第一次调用：第三方视角「岗位侧事实备忘录」，供正文写出「调研感」——与主邮件分两次请求（对齐 axioms：可验证、数据优于观点）
const JD_ALIGNMENT_SYSTEM_PROMPT: &str = r#"你是法律/投融资招聘场景的「岗位信息整理」助手。输出为**第三方视角**的结构化备忘录，供第二步邮件生成改写成第一人称——**不是求职信**，更不是自我评价。

硬性禁止：出现「我」「本人」「申请者」「契合」「匹配」「高度」「希望」「期待」「关注贵团队」「与我的」等词；禁止任何把岗位与申请人相连的句子。

根据用户给出的【岗位描述】，用 **约 150～320 个汉字**，严格输出**三段**，顺序固定。每一段第一行仅为标签行，第二行起写正文；段与段之间空一行。**不得**使用 Markdown 的 # 标题或编号列表。

【业务白话】
第二行起用 **2～4 句**。用第一性原理、**轻量**说明：在 JD 已给出的业务词范围内，该团队/条线的工作在实务中大致在解决什么问题（例如：仅在 JD 出现管线/并购/监管合规/数据合规等表述时，可对应到在研权利与对价、控制权与资源整合、药械环节规则遵守、健康数据与跨境边界等——**不得**展开 JD 未提及的细分赛道或外部案例）。**每一句都须能从 JD 用语中找到依据**，勿编造机构名、案例、数据或市场新闻。

【JD事实】
第二行起用 **4～8 个短句**。只写下列类型，且**每一句都须能在 JD 中找到依据**：团队/分所/业务条线名称或领域（尽量沿用 JD 原文用语）；核心业务动作或交易/服务类型；主要客户类型、行业或监管/合规场景；若 JD 写明实习生或初阶人员的工作内容、交付物、技能关键词则客观列出，未写则省略，勿臆测。

【岗位侧要求】
第二行起用 **0～3 句**。仅概括 JD **明文写出**的对实习生/初阶人员的出勤、时长、语言、专业背景、交付或技能要求；若 JD 完全未写可写「未写明」一句，勿臆测。

文风：像内部招聘调研笔记，**名词与动宾结构为主**；禁止形容词堆砌、禁止抒情与评价。"#;

const JD_ALIGNMENT_TEMPERATURE: f32 = 0.28;
const JD_ALIGNMENT_MAX_TOKENS: u32 = 720;
const MAX_JD_FOR_ALIGNMENT_CHARS: usize = 25_000;
const MAX_JD_ALIGNMENT_SYSTEM_PROMPT_CHARS: usize = 80_000;
const MAX_EMAIL_USER_SUFFIX_PROMPT_CHARS: usize = 120_000;

// #region agent log
/// `CARGO_MANIFEST_DIR` = `.../job-mailer/src-tauri` → 上一级 = `job-mailer` 包根（与 `src-tauri` 同级），日志落在仓库内便于 Cursor 读取。
fn job_mailer_package_debug_log_path() -> Option<PathBuf> {
    Some(Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.join("debug-9dc719.log"))
}

/// 仓库根下 `.cursor/`（`src-tauri` 的上级的上级）。若 Job Mailer 单独拷贝到别处，该路径可能不在当前 Cursor 工作区。
fn repo_root_cursor_debug_log_path() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.parent()?;
    Some(root.join(".cursor").join("debug-9dc719.log"))
}

/// `…/context-infrastructure/tmp/`（相对仓库根）。含 `context-infrastructure` 的 Cursor 工作区必能搜到。
fn context_infra_tmp_debug_log_path() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent()?.parent()?;
    Some(
        root
            .join("context-infrastructure")
            .join("tmp")
            .join("debug-9dc719.log"),
    )
}

fn agent_debug_log(hypothesis_id: &str, location: &str, message: &str, mut data: serde_json::Value) {
    let log_path = app_data_dir()
        .map(|d| d.join("debug-9dc719.log"))
        .unwrap_or_else(|_| PathBuf::from("/tmp/job-mailer-debug-9dc719.log"));
    if let Some(obj) = data.as_object_mut() {
        obj.insert(
            "log_path_app_support".to_string(),
            serde_json::Value::String(log_path.display().to_string()),
        );
        if let Some(ref p) = job_mailer_package_debug_log_path() {
            obj.insert(
                "log_path_job_mailer_pkg".to_string(),
                serde_json::Value::String(p.display().to_string()),
            );
        }
        if let Some(ref p) = repo_root_cursor_debug_log_path() {
            obj.insert(
                "log_path_repo_cursor".to_string(),
                serde_json::Value::String(p.display().to_string()),
            );
        }
        if let Some(ref p) = context_infra_tmp_debug_log_path() {
            obj.insert(
                "log_path_context_infra_tmp".to_string(),
                serde_json::Value::String(p.display().to_string()),
            );
        }
    }
    let payload = serde_json::json!({
        "sessionId": "9dc719",
        "hypothesisId": hypothesis_id,
        "location": location,
        "message": message,
        "data": data,
        "timestamp": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    });
    let line = payload.to_string();
    let mut paths: Vec<PathBuf> = vec![log_path.clone()];
    if let Some(p) = job_mailer_package_debug_log_path() {
        paths.push(p);
    }
    if let Some(p) = repo_root_cursor_debug_log_path() {
        paths.push(p);
    }
    if let Some(p) = context_infra_tmp_debug_log_path() {
        paths.push(p);
    }
    let mut wrote = 0u32;
    for p in &paths {
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p)
        {
            if writeln!(f, "{}", line).is_ok() {
                wrote += 1;
            }
        }
    }
    if wrote == 0 {
        eprintln!(
            "[job-mailer debug] agent_debug_log failed for all {} paths (first: {:?})",
            paths.len(),
            paths.first()
        );
    }
}
// #endregion

fn fluff_marker_hits(body: &str) -> Vec<&'static str> {
    FLUFF_MARKERS
        .iter()
        .copied()
        .filter(|m| body.contains(m))
        .collect()
}

/// 初稿含套话时第二次调用：压缩态度修辞，仍返回 JSON
async fn defluff_generated_email(
    client: &reqwest::Client,
    chat_url: &str,
    api_key: &str,
    model: &str,
    draft: &GenJson,
) -> Result<GenJson, String> {
    let draft_json =
        serde_json::to_string(draft).map_err(|e| format!("序列化初稿失败: {}", e))?;
    let user = format!(
        "【硬性禁止出现在改写稿中】\n书信：此致、敬礼。\n态度套话：求知欲、持续关注、前沿、抱有、希望能在、希望能、期待能有机会、期待您的审阅、进一步沟通、立志从事、高度的责任心、形成初步认知、注重培养、不断提升专业能力、主动进行法律研究与思考。\n其他：深感兴趣、我了解到、我关注贵团队、希望有机会、与贵团队高度契合、浓厚兴趣、学习热情。\n\n【初稿 JSON】\n{}\n\n只输出改写后的 JSON，字段 subject、body。",
        draft_json
    );
    let req = ChatCompletionRequest {
        model: model.to_string(),
        messages: vec![
            ChatMessage {
                role: "system".into(),
                content: DEFLUFF_SYSTEM_PROMPT.to_string(),
            },
            ChatMessage {
                role: "user".into(),
                content: user,
            },
        ],
        temperature: DEFLUFF_TEMPERATURE,
        max_tokens: Some(4096),
    };
    let text = fetch_chat_completion_content(client, chat_url, api_key, req).await?;
    let json_str = extract_json_object(&text);
    serde_json::from_str(&json_str).map_err(|e| format!("去套话 JSON 解析失败: {} — {}", e, json_str))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: String,
    pub created_at: i64,
    pub subject: String,
    pub body: String,
    pub jd_text: String,
    pub strategy_path: Option<String>,
    pub strategy_md: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppendHistoryInput {
    pub subject: String,
    pub body: String,
    pub jd_text: String,
    pub strategy_path: Option<String>,
    pub strategy_md: String,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct HistoryFile {
    entries: Vec<HistoryEntry>,
}

fn truncate_str(s: &str, max_chars: usize) -> String {
    let n = s.chars().count();
    if n <= max_chars {
        s.to_string()
    } else {
        s.chars()
            .take(max_chars.saturating_sub(1))
            .collect::<String>()
            + "…"
    }
}

#[tauri::command]
fn list_generation_history() -> Result<Vec<HistoryEntry>, String> {
    let path = history_path()?;
    if !path.exists() {
        return Ok(vec![]);
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut file: HistoryFile = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    file.entries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(file.entries)
}

#[tauri::command]
fn append_generation_history(input: AppendHistoryInput) -> Result<HistoryEntry, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    let entry = HistoryEntry {
        id,
        created_at,
        subject: input.subject.trim().to_string(),
        body: input.body,
        jd_text: truncate_str(&input.jd_text, MAX_JD_CHARS),
        strategy_path: input.strategy_path,
        strategy_md: truncate_str(&input.strategy_md, MAX_STRATEGY_CHARS),
    };

    let path = history_path()?;
    let mut file = if path.exists() {
        let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        serde_json::from_str::<HistoryFile>(&raw).unwrap_or_default()
    } else {
        HistoryFile::default()
    };

    file.entries.insert(0, entry.clone());
    if file.entries.len() > MAX_HISTORY_ENTRIES {
        file.entries.truncate(MAX_HISTORY_ENTRIES);
    }

    let raw = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    std::fs::write(path, raw).map_err(|e| e.to_string())?;
    Ok(entry)
}

#[tauri::command]
fn delete_generation_history(id: String) -> Result<(), String> {
    let path = history_path()?;
    if !path.exists() {
        return Ok(());
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut file: HistoryFile = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    file.entries.retain(|e| e.id != id);
    let raw = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    std::fs::write(path, raw).map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StoredSettings {
    #[serde(default)]
    pub llm_base_url: String,
    #[serde(default)]
    pub llm_api_key: String,
    #[serde(default)]
    pub llm_model: String,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub smtp_host: String,
    #[serde(default)]
    pub smtp_port: u16,
    #[serde(default)]
    pub smtp_username: String,
    #[serde(default)]
    pub smtp_password: String,
    #[serde(default)]
    pub from_email: String,
    #[serde(default)]
    pub resume_profile: String,
    /// 生成邮件前是否先调用一次模型，根据 JD 生成业务领域摘要（约百字量级，供首段参考）
    #[serde(default = "default_true")]
    pub enable_jd_alignment_digest: bool,
    /// 第一轮 API：JD 对齐的 **system**；空则使用内置 `JD_ALIGNMENT_SYSTEM_PROMPT`
    #[serde(default)]
    pub jd_alignment_system_prompt: String,
    /// 第二轮 API：拼在 user 消息**末尾**的硬性约束；空则使用内置 `USER_GENERATION_TAIL`
    #[serde(default)]
    pub email_user_suffix_prompt: String,
}

fn default_true() -> bool {
    true
}

#[tauri::command]
fn load_settings() -> Result<StoredSettings, String> {
    let path = app_settings_path()?;
    if !path.exists() {
        return Ok(StoredSettings {
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
            smtp_port: 587,
            enable_jd_alignment_digest: true,
            jd_alignment_system_prompt: String::new(),
            email_user_suffix_prompt: String::new(),
            ..Default::default()
        });
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut s: StoredSettings = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let file_had_empty_system_prompt = s.system_prompt.trim().is_empty();
    if file_had_empty_system_prompt {
        s.system_prompt = DEFAULT_SYSTEM_PROMPT.to_string();
    }
    if s.smtp_port == 0 {
        s.smtp_port = 587;
    }
    Ok(s)
}

#[tauri::command]
fn save_settings(settings: StoredSettings) -> Result<(), String> {
    let path = app_settings_path()?;
    let raw = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    std::fs::write(path, raw).map_err(|e| e.to_string())
}

#[tauri::command]
fn read_text_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| e.to_string())
}

fn normalize_base_url(base: &str) -> String {
    base.trim().trim_end_matches('/').to_string()
}

/// 首行是否像邮件称呼（用于补全判断）
fn first_line_looks_like_salutation(first_line: &str) -> bool {
    let s = first_line.trim();
    if s.is_empty() {
        return false;
    }
    s.contains("您好")
        || s.contains("尊敬")
        || s.contains("老师")
        || s.starts_with("Dear ")
        || (s.contains("律师") && s.len() <= 48)
        || s.ends_with('：')
        || s.ends_with(':')
}

fn ensure_body_salutation(body: &str) -> (String, bool) {
    let t = body.trim();
    if t.is_empty() {
        return (String::new(), false);
    }
    let first = t.lines().next().unwrap_or("").trim();
    if first_line_looks_like_salutation(first) {
        (t.to_string(), false)
    } else {
        (format!("{}{}", DEFAULT_SALUTATION_PREFIX, t), true)
    }
}

/// 去掉模型偶发的「此致 / 敬礼」书信尾（与 USER_GENERATION_TAIL 一致）
fn strip_traditional_letter_closing(body: &str) -> String {
    let t = body.trim_end();
    let mut out = t.to_string();
    let suffixes = [
        "\n\n此致\n敬礼",
        "\n此致\n敬礼",
        "\n\n此致\n\n敬礼",
        "\n\n此致 \n敬礼",
    ];
    for suf in suffixes {
        if out.ends_with(suf) {
            out = out[..out.len() - suf.len()].trim_end().to_string();
            break;
        }
    }
    out
}

fn extract_json_object(raw: &str) -> String {
    let s = raw.trim();
    if let (Some(i), Some(j)) = (s.find('{'), s.rfind('}')) {
        if j >= i {
            return s[i..=j].to_string();
        }
    }
    s.to_string()
}

async fn fetch_chat_completion_content(
    client: &reqwest::Client,
    chat_url: &str,
    api_key: &str,
    req_body: ChatCompletionRequest,
) -> Result<String, String> {
    let resp = client
        .post(chat_url)
        .header(
            "Authorization",
            format!("Bearer {}", api_key.trim()),
        )
        .header("Content-Type", "application/json")
        .json(&req_body)
        .send()
        .await
        .map_err(|e| format!("请求失败: {}", e))?;

    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("API 错误 {}: {}", status, text));
    }

    let parsed: ChatCompletionResponse =
        serde_json::from_str(&text).map_err(|e| format!("解析响应 JSON 失败: {} — 原文: {}", e, text))?;

    let content = parsed
        .choices
        .first()
        .ok_or_else(|| "响应无 choices".to_string())?
        .message
        .content
        .clone();

    Ok(content)
}

/// 第一次 API：从 JD 提炼业务领域摘要，供主邮件首段参考
async fn generate_jd_alignment_paragraph(
    client: &reqwest::Client,
    chat_url: &str,
    api_key: &str,
    model: &str,
    jd_excerpt: &str,
    system_prompt_override: &str,
) -> Result<String, String> {
    if jd_excerpt.trim().is_empty() {
        return Ok(String::new());
    }
    let system_content = if system_prompt_override.trim().is_empty() {
        JD_ALIGNMENT_SYSTEM_PROMPT.to_string()
    } else {
        system_prompt_override.to_string()
    };
    let req = ChatCompletionRequest {
        model: model.to_string(),
        messages: vec![
            ChatMessage {
                role: "system".into(),
                content: system_content,
            },
            ChatMessage {
                role: "user".into(),
                content: format!("【岗位描述】\n{}", jd_excerpt),
            },
        ],
        temperature: JD_ALIGNMENT_TEMPERATURE,
        max_tokens: Some(JD_ALIGNMENT_MAX_TOKENS),
    };
    let content = fetch_chat_completion_content(client, chat_url, api_key, req).await?;
    // #region agent log
    agent_debug_log(
        "H1",
        "lib.rs:generate_jd_alignment_paragraph",
        "alignment_api_ok",
        serde_json::json!({
            "alignment_chars": content.chars().count(),
            "jd_excerpt_chars": jd_excerpt.chars().count(),
        }),
    );
    // #endregion
    Ok(content)
}

#[tauri::command]
async fn generate_email(
    llm: LlmConfig,
    jd_text: String,
    strategy_markdown: String,
    resume_profile: String,
    enable_jd_alignment_digest: bool,
    jd_alignment_system_prompt: String,
    email_user_suffix_prompt: String,
) -> Result<GeneratedEmail, String> {
    let base = normalize_base_url(&llm.base_url);
    let url = format!("{}/chat/completions", base);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| e.to_string())?;

    // #region agent log
    agent_debug_log(
        "H0",
        "lib.rs:generate_email",
        "invoke",
        serde_json::json!({
            "jd_chars": jd_text.chars().count(),
            "strategy_chars": strategy_markdown.chars().count(),
            "resume_chars": resume_profile.chars().count(),
            "enable_jd_alignment_digest": enable_jd_alignment_digest,
        }),
    );
    // #endregion

    let api_key = llm.api_key.clone();
    let model_id = llm.model.clone();

    let jd_align_sys =
        truncate_str(jd_alignment_system_prompt.trim(), MAX_JD_ALIGNMENT_SYSTEM_PROMPT_CHARS);
    let jd_alignment = if enable_jd_alignment_digest {
        let jd_for_alignment = truncate_str(jd_text.trim(), MAX_JD_FOR_ALIGNMENT_CHARS);
        generate_jd_alignment_paragraph(
            &client,
            &url,
            api_key.trim(),
            model_id.trim(),
            &jd_for_alignment,
            jd_align_sys.as_str(),
        )
        .await
        .unwrap_or_default()
    } else {
        String::new()
    };

    // #region agent log
    agent_debug_log(
        "H1",
        "lib.rs:generate_email",
        "after_alignment_step",
        serde_json::json!({
            "enable_jd_alignment_digest": enable_jd_alignment_digest,
            "alignment_empty": jd_alignment.trim().is_empty(),
            "alignment_chars": jd_alignment.chars().count(),
        }),
    );
    // #endregion

    let alignment_block = if jd_alignment.trim().is_empty() {
        String::new()
    } else {
        format!(
            "【岗位侧事实备忘录（第一轮预生成；含「业务白话」「JD事实」「岗位侧要求」三段；写入正文时改写成第一人称邮件语气，融入 system「内置准备模块」，勿逐句照抄标签）】\n{}\n\n",
            jd_alignment.trim()
        )
    };

    let uses_default_system = llm.system_prompt.trim().is_empty();
    let system = if uses_default_system {
        DEFAULT_SYSTEM_PROMPT.to_string()
    } else {
        llm.system_prompt
    };

    let resume = truncate_str(resume_profile.trim(), MAX_RESUME_PROFILE_CHARS);
    let tail_block: String = if email_user_suffix_prompt.trim().is_empty() {
        USER_GENERATION_TAIL.to_string()
    } else {
        truncate_str(
            email_user_suffix_prompt.trim(),
            MAX_EMAIL_USER_SUFFIX_PROMPT_CHARS,
        )
    };
    let user_content = if resume.is_empty() {
        format!(
            "{}{}【岗位描述 / 团队信息】\n{}\n\n【投递策略资产（Markdown）】\n{}{}",
            USER_TASK_PREAMBLE,
            alignment_block,
            jd_text.trim(),
            strategy_markdown.trim(),
            tail_block
        )
    } else {
        format!(
            "{}{}【个人简介（仅用于组织标题与身份表述，勿逐字照抄全文）】\n{}\n\n【岗位描述 / 团队信息】\n{}\n\n【投递策略资产（Markdown）】\n{}{}",
            USER_TASK_PREAMBLE,
            alignment_block,
            resume,
            jd_text.trim(),
            strategy_markdown.trim(),
            tail_block
        )
    };

    // #region agent log
    let strategy_placeholder = strategy_markdown.trim().starts_with("（未选择策略文件");
    agent_debug_log(
        "H2-H5",
        "lib.rs:generate_email",
        "before_main_completion",
        serde_json::json!({
            "uses_default_system_prompt": uses_default_system,
            "system_chars": system.chars().count(),
            "user_content_chars": user_content.chars().count(),
            "strategy_placeholder": strategy_placeholder,
            "strategy_chars": strategy_markdown.chars().count(),
            "jd_chars": jd_text.chars().count(),
            "resume_chars": resume.chars().count(),
        }),
    );
    // #endregion

    let body = ChatCompletionRequest {
        model: model_id.clone(),
        messages: vec![
            ChatMessage {
                role: "system".into(),
                content: system,
            },
            ChatMessage {
                role: "user".into(),
                content: user_content,
            },
        ],
        temperature: GENERATION_TEMPERATURE,
        max_tokens: None,
    };

    let text = fetch_chat_completion_content(&client, &url, api_key.trim(), body).await?;

    let json_str = extract_json_object(&text);
    let mut gen: GenJson =
        serde_json::from_str(&json_str).map_err(|e| format!("解析邮件 JSON 失败: {} — 片段: {}", e, json_str))?;

    let body_trimmed = gen.body.trim().to_string();
    let (mut body_final, _) = ensure_body_salutation(&body_trimmed);

    // #region agent log
    let fluff_hits_pre = fluff_marker_hits(&body_final);
    agent_debug_log(
        "H6",
        "lib.rs:generate_email",
        "output_fluff_heuristic_pre_defluff",
        serde_json::json!({
            "body_chars": body_final.chars().count(),
            "fluff_marker_hits": fluff_hits_pre.len(),
            "markers": fluff_hits_pre,
        }),
    );

    // 主生成后**始终**跑一次去套话：启发式未命中时仍常有「期待/此致/敬礼」等（见日志 L12：H6=0 但正文质量差）。
    let draft = GenJson {
        subject: gen.subject.trim().to_string(),
        body: body_final.clone(),
    };
    match defluff_generated_email(&client, &url, api_key.trim(), model_id.trim(), &draft).await {
        Ok(d) => {
            gen = GenJson {
                subject: d.subject.trim().to_string(),
                body: d.body.trim().to_string(),
            };
            let bt = gen.body.trim().to_string();
            let (bf, _) = ensure_body_salutation(&bt);
            body_final = strip_traditional_letter_closing(&bf);
            let post_hits = fluff_marker_hits(&body_final);
            agent_debug_log(
                "H7",
                "lib.rs:generate_email",
                "defluff_applied",
                serde_json::json!({
                    "post_fluff_hits": post_hits.len(),
                    "markers": post_hits,
                }),
            );
        }
        Err(e) => {
            body_final = strip_traditional_letter_closing(&body_final);
            agent_debug_log(
                "H7",
                "lib.rs:generate_email",
                "defluff_skipped_error",
                serde_json::json!({ "error": e }),
            );
        }
    }
    // #endregion

    Ok(GeneratedEmail {
        subject: gen.subject.trim().to_string(),
        body: body_final,
    })
}

fn send_email_smtp_sync(
    smtp: SmtpConfig,
    to: String,
    subject: String,
    body: String,
    attachment_path: Option<String>,
) -> Result<(), String> {
    let from_mb = Mailbox::from_str(smtp.from_email.trim()).map_err(|e| format!("发件人格式无效: {}", e))?;
    let to_mb = Mailbox::from_str(to.trim()).map_err(|e| format!("收件人格式无效: {}", e))?;

    let email = build_message(from_mb, to_mb, &subject, &body, attachment_path.as_deref())?;

    let creds = Credentials::new(smtp.username.clone(), smtp.password.clone());
    let host = smtp.host.trim();
    let port = smtp.port;

    // `relay()` = 465 SMTPS（隐式 TLS）；`starttls_relay()` = 587 等 STARTTLS。仅改 port 不改 TLS 会导致 Gmail 587 卡住。
    let builder = if port == 465 {
        SmtpTransport::relay(host).map_err(|e| format!("SMTP（SMTPS）配置失败: {}", e))?
    } else {
        SmtpTransport::starttls_relay(host).map_err(|e| format!("SMTP（STARTTLS）配置失败: {}", e))?
    };

    let transport = builder
        .credentials(creds)
        .port(port)
        .timeout(Some(SMTP_IO_TIMEOUT))
        .build();

    transport
        .send(&email)
        .map_err(|e| format!("发送失败: {}", e))?;

    Ok(())
}

#[tauri::command]
async fn send_email_smtp(
    smtp: SmtpConfig,
    to: String,
    subject: String,
    body: String,
    attachment_path: Option<String>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        send_email_smtp_sync(smtp, to, subject, body, attachment_path)
    })
    .await
    .map_err(|e| format!("发送任务异常: {}", e))?
}

fn build_message(
    from: Mailbox,
    to: Mailbox,
    subject: &str,
    body: &str,
    attachment_path: Option<&str>,
) -> Result<Message, String> {
    let builder = Message::builder().from(from).to(to).subject(subject);

    if let Some(p) = attachment_path {
        let p = p.trim();
        if !p.is_empty() {
            let path = Path::new(p);
            let bytes = std::fs::read(path).map_err(|e| format!("读取附件失败: {}", e))?;
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("attachment.pdf")
                .to_string();
            let ct = ContentType::parse("application/pdf").map_err(|e| e.to_string())?;
            let attachment = Attachment::new(name).body(bytes, ct);

            let text_part = SinglePart::plain(body.to_string());

            let multipart = MultiPart::mixed()
                .singlepart(text_part)
                .singlepart(attachment);

            return builder.multipart(multipart).map_err(|e| e.to_string());
        }
    }

    builder.body(body.to_string()).map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            read_text_file,
            generate_email,
            send_email_smtp,
            load_settings,
            save_settings,
            list_generation_history,
            append_generation_history,
            delete_generation_history,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod agent_log_tests {
    use super::*;

    #[test]
    fn debug_log_writes_under_context_infra_tmp() {
        if let Some(p) = context_infra_tmp_debug_log_path() {
            let _ = std::fs::remove_file(&p);
        }
        agent_debug_log(
            "TEST",
            "lib.rs:agent_log_tests",
            "write_smoke",
            serde_json::json!({ "smoke": true }),
        );
        let p = context_infra_tmp_debug_log_path()
            .expect("repo root must be two levels above CARGO_MANIFEST_DIR");
        assert!(p.is_file(), "log missing at {}", p.display());
        let raw = std::fs::read_to_string(&p).expect("read log");
        assert!(
            raw.contains("write_smoke"),
            "unexpected log content: {}",
            raw
        );
    }
}
