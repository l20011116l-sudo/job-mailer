use lettre::message::header::ContentType;
use lettre::message::{Attachment, Mailbox, Message, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{SmtpTransport, Transport};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

/// 内置 prompt 已清空；请在设置中填写 system / user 末尾补充，便于自行做 prompt 实验。

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

/// OpenAI 兼容请求中的联网选项；LiteLLM 会映射为 Gemini 的 Google Search Grounding（见 LiteLLM Web Search 文档）。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WebSearchOptionsBody {
    search_context_size: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    web_search_options: Option<WebSearchOptionsBody>,
}

fn normalize_web_search_context_size(raw: &str) -> String {
    match raw.trim().to_lowercase().as_str() {
        "low" => "low".to_string(),
        "high" => "high".to_string(),
        _ => "medium".to_string(),
    }
}

/// 主生成请求是否附带 `web_search_options`（LiteLLM → Gemini 等）。
fn web_search_options_if_enabled(enabled: bool, context_size: &str) -> Option<WebSearchOptionsBody> {
    if !enabled {
        return None;
    }
    Some(WebSearchOptionsBody {
        search_context_size: normalize_web_search_context_size(context_size),
    })
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

/// 模型未生成称呼时，由程序补全
const DEFAULT_SALUTATION_PREFIX: &str = "您好，\n\n";

const GENERATION_TEMPERATURE: f32 = 0.38;

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
    /// 拼在 user 消息**末尾**（空则不加）
    #[serde(default)]
    pub email_user_suffix_prompt: String,
    /// 在 chat/completions 请求中附加 `web_search_options`（LiteLLM → Gemini Google Search）；非 Gemini 时建议网关 `drop_params`
    #[serde(default)]
    pub enable_gemini_google_search: bool,
    /// `low` / `medium` / `high`
    #[serde(default = "default_gemini_web_search_context_size")]
    pub gemini_web_search_context_size: String,
}

fn default_gemini_web_search_context_size() -> String {
    "medium".to_string()
}

#[tauri::command]
fn load_settings() -> Result<StoredSettings, String> {
    let path = app_settings_path()?;
    if !path.exists() {
        return Ok(StoredSettings {
            smtp_port: 587,
            email_user_suffix_prompt: String::new(),
            enable_gemini_google_search: false,
            gemini_web_search_context_size: default_gemini_web_search_context_size(),
            ..Default::default()
        });
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut s: StoredSettings = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
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

/// 去掉模型偶发的「此致 / 敬礼」书信尾
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

#[tauri::command]
async fn generate_email(
    llm: LlmConfig,
    jd_text: String,
    strategy_markdown: String,
    resume_profile: String,
    email_user_suffix_prompt: String,
    enable_gemini_google_search: bool,
    gemini_web_search_context_size: String,
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
            "enable_gemini_google_search": enable_gemini_google_search,
            "system_empty": llm.system_prompt.trim().is_empty(),
        }),
    );
    // #endregion

    let api_key = llm.api_key.clone();
    let model_id = llm.model.clone();

    let ctx_sz = gemini_web_search_context_size.as_str();
    let ws_main = web_search_options_if_enabled(enable_gemini_google_search, ctx_sz);

    let system = llm.system_prompt;
    let tail_block = truncate_str(
        email_user_suffix_prompt.trim(),
        MAX_EMAIL_USER_SUFFIX_PROMPT_CHARS,
    );
    let suffix_part = if tail_block.is_empty() {
        String::new()
    } else {
        format!("\n\n{}", tail_block)
    };

    let resume = truncate_str(resume_profile.trim(), MAX_RESUME_PROFILE_CHARS);
    let user_content = if resume.is_empty() {
        format!(
            "【岗位描述 / 团队信息】\n{}\n\n【投递策略资产（Markdown）】\n{}{}",
            jd_text.trim(),
            strategy_markdown.trim(),
            suffix_part
        )
    } else {
        format!(
            "【个人简介】\n{}\n\n【岗位描述 / 团队信息】\n{}\n\n【投递策略资产（Markdown）】\n{}{}",
            resume,
            jd_text.trim(),
            strategy_markdown.trim(),
            suffix_part
        )
    };

    // #region agent log
    let strategy_placeholder = strategy_markdown.trim().starts_with("（未选择策略文件");
    agent_debug_log(
        "H2",
        "lib.rs:generate_email",
        "before_completion",
        serde_json::json!({
            "system_chars": system.chars().count(),
            "user_content_chars": user_content.chars().count(),
            "strategy_placeholder": strategy_placeholder,
            "strategy_chars": strategy_markdown.chars().count(),
            "jd_chars": jd_text.chars().count(),
            "resume_chars": resume.chars().count(),
        }),
    );
    // #endregion

    let mut messages: Vec<ChatMessage> = Vec::new();
    if !system.trim().is_empty() {
        messages.push(ChatMessage {
            role: "system".into(),
            content: system,
        });
    }
    messages.push(ChatMessage {
        role: "user".into(),
        content: user_content,
    });

    let body = ChatCompletionRequest {
        model: model_id.clone(),
        messages,
        temperature: GENERATION_TEMPERATURE,
        max_tokens: None,
        web_search_options: ws_main,
    };

    let text = fetch_chat_completion_content(&client, &url, api_key.trim(), body).await?;

    let json_str = extract_json_object(&text);
    let gen: GenJson =
        serde_json::from_str(&json_str).map_err(|e| format!("解析邮件 JSON 失败: {} — 片段: {}", e, json_str))?;

    let body_trimmed = gen.body.trim().to_string();
    let (body_sal, _) = ensure_body_salutation(&body_trimmed);
    let body_final = strip_traditional_letter_closing(&body_sal);

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
