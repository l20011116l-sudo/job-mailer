import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { LLM_PRESETS } from "./llmPresets";
import {
  PROMPT_EXAMPLE_BODY,
  PROMPT_EXAMPLE_NOTE,
  PROMPT_EXAMPLE_SUBJECT,
  PROMPT_LAYOUT_BLURB,
  REFERENCE_SYSTEM_PROMPT,
  REFERENCE_USER_SUFFIX_PROMPT,
} from "./promptExample";

type StoredSettings = {
  llmBaseUrl: string;
  llmApiKey: string;
  llmModel: string;
  /** 单次调用的 system；空则请求中不包含 system 消息 */
  systemPrompt: string;
  /** 拼在 user 末尾（岗位描述与策略之后）；空则不加 */
  emailUserSuffixPrompt: string;
  resumeProfile: string;
  /** 请求体附加 web_search_options（LiteLLM + Gemini → Google Search Grounding） */
  enableGeminiGoogleSearch: boolean;
  geminiWebSearchContextSize: "low" | "medium" | "high";
  smtpHost: string;
  smtpPort: number;
  smtpUsername: string;
  smtpPassword: string;
  fromEmail: string;
};

const emptySettings = (): StoredSettings => ({
  llmBaseUrl: "",
  llmApiKey: "",
  llmModel: "",
  systemPrompt: "",
  emailUserSuffixPrompt: "",
  resumeProfile: "",
  enableGeminiGoogleSearch: false,
  geminiWebSearchContextSize: "medium",
  smtpHost: "",
  smtpPort: 587,
  smtpUsername: "",
  smtpPassword: "",
  fromEmail: "",
});

type HistoryEntry = {
  id: string;
  createdAt: number;
  subject: string;
  body: string;
  jdText: string;
  strategyPath: string | null;
  strategyMd: string;
};

function formatTime(ms: number) {
  try {
    return new Date(ms).toLocaleString("zh-CN", {
      dateStyle: "short",
      timeStyle: "short",
    });
  } catch {
    return String(ms);
  }
}

/** 与常见招聘页粘贴文本兼容；边界用 \b 减少误匹配 */
const EMAIL_REGEX = /\b[A-Za-z0-9._%+-]+@[A-Za-z0-9][A-Za-z0-9.-]*\.[A-Za-z]{2,}\b/g;

function extractEmails(text: string): string[] {
  const seen = new Set<string>();
  const list: string[] = [];
  for (const m of text.matchAll(EMAIL_REGEX)) {
    const e = m[0];
    const key = e.toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    list.push(e);
  }
  return list;
}

function isLikelyAutomatedLocalPart(local: string): boolean {
  return /^(no-?reply|donotreply|noreply|mailer-daemon|bounce|postmaster|notifications?)$/i.test(
    local.trim(),
  );
}

function normalizeSearchContextSize(
  raw: string | undefined,
): "low" | "medium" | "high" {
  const t = (raw ?? "").trim().toLowerCase();
  if (t === "low" || t === "high") return t;
  return "medium";
}

function pickRecipientEmail(candidates: string[]): string | null {
  if (candidates.length === 0) return null;
  const human = candidates.find((c) => {
    const local = c.split("@")[0] ?? "";
    return !isLikelyAutomatedLocalPart(local);
  });
  return human ?? candidates[0];
}

function App() {
  const [tab, setTab] = useState<"compose" | "history" | "settings">("compose");
  const [settings, setSettings] = useState<StoredSettings>(emptySettings);
  const [loadError, setLoadError] = useState<string | null>(null);

  const [jdText, setJdText] = useState("");
  const [strategyPath, setStrategyPath] = useState<string | null>(null);
  const [strategyMd, setStrategyMd] = useState("");
  const [subject, setSubject] = useState("");
  const [body, setBody] = useState("");
  const [toEmail, setToEmail] = useState("");
  const [attachmentPath, setAttachmentPath] = useState<string | null>(null);

  const [busy, setBusy] = useState<string | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [history, setHistory] = useState<HistoryEntry[]>([]);

  const refreshHistory = useCallback(async () => {
    try {
      const list = await invoke<HistoryEntry[]>("list_generation_history");
      setHistory(list);
    } catch {
      setHistory([]);
    }
  }, []);

  useEffect(() => {
    (async () => {
      try {
        const s = await invoke<StoredSettings>("load_settings");
        setSettings({
          ...s,
          emailUserSuffixPrompt: s.emailUserSuffixPrompt ?? "",
          enableGeminiGoogleSearch: s.enableGeminiGoogleSearch ?? false,
          geminiWebSearchContextSize: normalizeSearchContextSize(s.geminiWebSearchContextSize),
        });
        setLoadError(null);
      } catch (e) {
        setLoadError(String(e));
      }
    })();
  }, []);

  useEffect(() => {
    if (tab === "history") {
      void refreshHistory();
    }
  }, [tab, refreshHistory]);

  /** 从岗位描述、策略正文中识别邮箱并填入收件人（不覆盖已填写内容） */
  useEffect(() => {
    const id = window.setTimeout(() => {
      const combined = `${jdText}\n${strategyMd}`;
      const emails = extractEmails(combined);
      const chosen = pickRecipientEmail(emails);
      if (!chosen) return;
      setToEmail((prev) => {
        if (prev.trim() !== "") return prev;
        return chosen;
      });
    }, 400);
    return () => clearTimeout(id);
  }, [jdText, strategyMd]);

  const showToast = useCallback((msg: string) => {
    setToast(msg);
    window.setTimeout(() => setToast(null), 3200);
  }, []);

  const copyPromptRef = useCallback(
    async (text: string, label: string) => {
      try {
        await navigator.clipboard.writeText(text);
        showToast(`已复制：${label}`);
      } catch {
        showToast("复制失败，请手动选择文本复制");
      }
    },
    [showToast],
  );

  const pickStrategy = async () => {
    const path = await open({
      multiple: false,
      filters: [{ name: "Markdown", extensions: ["md", "markdown", "txt"] }],
    });
    if (typeof path !== "string" || !path) return;
    setStrategyPath(path);
    try {
      const text = await invoke<string>("read_text_file", { path });
      setStrategyMd(text);
      showToast("已载入策略文件");
    } catch (e) {
      showToast(`读取失败: ${e}`);
    }
  };

  const pickPdf = async () => {
    const path = await open({
      multiple: false,
      filters: [{ name: "PDF", extensions: ["pdf"] }],
    });
    if (typeof path !== "string" || !path) return;
    setAttachmentPath(path);
    showToast("已选择简历 PDF");
  };

  const generate = async () => {
    if (!settings.llmBaseUrl.trim() || !settings.llmApiKey.trim() || !settings.llmModel.trim()) {
      showToast("请先在「设置」中填写 API 地址、Key 与模型名");
      setTab("settings");
      return;
    }
    if (!jdText.trim()) {
      showToast("请粘贴岗位描述或团队信息");
      return;
    }
    setBusy("正在生成…");
    try {
      const result = await invoke<{ subject: string; body: string }>("generate_email", {
        llm: {
          baseUrl: settings.llmBaseUrl,
          apiKey: settings.llmApiKey,
          model: settings.llmModel,
          systemPrompt: settings.systemPrompt,
        },
        jdText,
        strategyMarkdown:
          strategyMd.trim() ||
          "（未选择策略文件：无独立论据资产。第二段仅允许对齐 JD/个人简介中已有信息，并诚实说明具体经历见简历附件；禁止虚构项目；勿用「学习热情」等空话凑篇幅。）",
        resumeProfile: settings.resumeProfile,
        emailUserSuffixPrompt: settings.emailUserSuffixPrompt,
        enableGeminiGoogleSearch: settings.enableGeminiGoogleSearch,
        geminiWebSearchContextSize: settings.geminiWebSearchContextSize,
      });
      setSubject(result.subject);
      setBody(result.body);
      try {
        await invoke("append_generation_history", {
          subject: result.subject,
          body: result.body,
          jdText,
          strategyPath,
          strategyMd,
        });
        void refreshHistory();
      } catch {
        /* 历史写入失败不阻断主流程 */
      }
      showToast("生成完成，已记入本地历史；请人工核对后再发送。");
    } catch (e) {
      showToast(`生成失败: ${e}`);
    } finally {
      setBusy(null);
    }
  };

  const sendMail = async () => {
    if (!settings.smtpHost.trim() || !settings.smtpUsername.trim() || !settings.fromEmail.trim()) {
      showToast("请先在「设置」中填写 SMTP 与发件人");
      setTab("settings");
      return;
    }
    if (!toEmail.trim()) {
      showToast("请填写收件人邮箱");
      return;
    }
    if (!subject.trim() || !body.trim()) {
      showToast("请填写标题与正文");
      return;
    }
    setBusy("正在发送…");
    try {
      await invoke("send_email_smtp", {
        smtp: {
          host: settings.smtpHost,
          port: settings.smtpPort || 587,
          username: settings.smtpUsername,
          password: settings.smtpPassword,
          fromEmail: settings.fromEmail,
        },
        to: toEmail.trim(),
        subject: subject.trim(),
        body: body,
        attachmentPath: attachmentPath && attachmentPath.length > 0 ? attachmentPath : null,
      });
      showToast("已发送");
    } catch (e) {
      showToast(`发送失败: ${e}`);
    } finally {
      setBusy(null);
    }
  };

  const saveSettings = async () => {
    setBusy("保存中…");
    try {
      await invoke("save_settings", { settings });
      showToast("设置已保存到本机");
    } catch (e) {
      showToast(`保存失败: ${e}`);
    } finally {
      setBusy(null);
    }
  };

  const copyAll = async () => {
    const text = `标题: ${subject}\n\n${body}`;
    await navigator.clipboard.writeText(text);
    showToast("已复制到剪贴板");
  };

  const saveCurrentToHistory = async () => {
    if (!subject.trim() && !body.trim()) {
      showToast("没有可保存的标题或正文");
      return;
    }
    setBusy("保存历史…");
    try {
      await invoke("append_generation_history", {
        subject: subject.trim() || "（无标题）",
        body,
        jdText,
        strategyPath,
        strategyMd,
      });
      void refreshHistory();
      showToast("已保存到本地历史");
    } catch (e) {
      showToast(`保存失败: ${e}`);
    } finally {
      setBusy(null);
    }
  };

  const applyHistory = (h: HistoryEntry) => {
    setJdText(h.jdText);
    setSubject(h.subject);
    setBody(h.body);
    setStrategyPath(h.strategyPath);
    setStrategyMd(h.strategyMd);
    setTab("compose");
    showToast("已载入撰写区，请核对收件人与附件后发送");
  };

  const removeHistory = async (id: string) => {
    try {
      await invoke("delete_generation_history", { id });
      void refreshHistory();
      showToast("已删除该条");
    } catch (e) {
      showToast(`删除失败: ${e}`);
    }
  };

  const applyPreset = (baseUrl: string, modelPlaceholder: string) => {
    setSettings((prev) => ({
      ...prev,
      llmBaseUrl: baseUrl,
      llmModel: prev.llmModel.trim() ? prev.llmModel : modelPlaceholder,
    }));
    showToast("已填入 Base URL" + (modelPlaceholder ? `；模型名可填：${modelPlaceholder}` : ""));
  };

  return (
    <div className="app">
      <header className="top">
        <h1 className="title">Job Mailer</h1>
        <nav className="tabs">
          <button type="button" className={tab === "compose" ? "on" : ""} onClick={() => setTab("compose")}>
            撰写
          </button>
          <button type="button" className={tab === "history" ? "on" : ""} onClick={() => setTab("history")}>
            历史
          </button>
          <button type="button" className={tab === "settings" ? "on" : ""} onClick={() => setTab("settings")}>
            设置
          </button>
        </nav>
      </header>

      {loadError && <p className="banner warn">加载设置失败: {loadError}</p>}
      {toast && <p className="banner ok">{toast}</p>}
      {busy && <p className="banner muted">{busy}</p>}

      {tab === "compose" && (
        <main className="main compose">
          <section className="panel">
            <h2>岗位与策略</h2>
            <label className="label">岗位描述 / 团队信息（从招聘页粘贴）</label>
            <textarea
              className="field tall"
              value={jdText}
              onChange={(e) => setJdText(e.target.value)}
              placeholder="职位要求、团队领域、联系方式等"
              spellCheck={false}
            />
            <div className="row gap">
              <button type="button" className="btn secondary" onClick={pickStrategy}>
                选择策略 Markdown
              </button>
              {strategyPath && (
                <span className="hint mono">{strategyPath}</span>
              )}
            </div>
            {strategyMd ? (
              <p className="hint">策略已载入，约 {strategyMd.length} 字。</p>
            ) : (
              <p className="hint">
                未选策略文件时：撰写区仍会附带占位说明；具体论证与语气完全由你在「设置」里的 prompt 决定。
              </p>
            )}
          </section>

          <section className="panel">
            <h2>生成与审核</h2>
            <p className="hint">
              每次点击「生成邮件」仅发起<strong>一次</strong>模型请求；回复仍须为含 <code className="mono">subject</code>、<code className="mono">body</code> 的
              JSON（由你在设置中的 system / 约束里说明）。若需换 prompt 做对比，请在设置里修改后保存再生成。
            </p>
            <div className="row gap wrap">
              <button type="button" className="btn primary" onClick={generate} disabled={!!busy}>
                生成邮件
              </button>
              <button type="button" className="btn secondary" onClick={copyAll}>
                复制标题与正文
              </button>
              <button type="button" className="btn secondary" onClick={saveCurrentToHistory} disabled={!!busy}>
                保存当前到历史
              </button>
            </div>
            <label className="label">标题</label>
            <p className="hint">
              生成后自动填入。若在「设置」中填写了个人简介，模型会据此组织主题行（姓名、院校、学历等），无需在撰写页重复输入。
            </p>
            <input
              className="field"
              value={subject}
              onChange={(e) => setSubject(e.target.value)}
              placeholder="点击「生成邮件」后自动填充，仍可手改"
            />
            <label className="label">正文</label>
            <textarea
              className="field tall"
              value={body}
              onChange={(e) => setBody(e.target.value)}
              placeholder="生成后可在此修改"
              spellCheck={false}
            />
            <label className="label">收件人</label>
            <p className="hint">
              岗位描述或策略正文中如出现邮箱，会自动填入此处；若你已填写收件人则不会覆盖。
            </p>
            <input
              className="field"
              value={toEmail}
              onChange={(e) => setToEmail(e.target.value)}
              placeholder="团队或 HR 邮箱"
              type="email"
            />
            <div className="row gap wrap">
              <button type="button" className="btn secondary" onClick={pickPdf}>
                附加简历 PDF
              </button>
              {attachmentPath && <span className="hint mono">{attachmentPath}</span>}
            </div>
            <div className="row gap wrap send-row">
              <button type="button" className="btn send" onClick={sendMail} disabled={!!busy}>
                确认发送
              </button>
            </div>
          </section>
        </main>
      )}

      {tab === "history" && (
        <main className="main history">
          <section className="panel">
            <h2>生成历史（本机）</h2>
            <p className="hint">最多保留 50 条；同一业务多律所时可载入后改收件人与正文再发。文件路径若仍存在，策略正文会一并恢复。</p>
            {history.length === 0 && <p className="hint">暂无记录。生成邮件后会自动追加，也可在撰写页点击「保存当前到历史」。</p>}
            <ul className="history-list">
              {history.map((h) => (
                <li key={h.id} className="history-card">
                  <div className="history-meta">
                    <span className="history-time">{formatTime(h.createdAt)}</span>
                    <span className="history-subject">{h.subject || "（无标题）"}</span>
                  </div>
                  <p className="history-preview">{h.jdText.slice(0, 160)}{h.jdText.length > 160 ? "…" : ""}</p>
                  <div className="row gap wrap history-actions">
                    <button type="button" className="btn primary" onClick={() => applyHistory(h)}>
                      载入到撰写
                    </button>
                    <button type="button" className="btn secondary" onClick={() => removeHistory(h.id)}>
                      删除
                    </button>
                  </div>
                </li>
              ))}
            </ul>
          </section>
        </main>
      )}

      {tab === "settings" && (
        <main className="main settings">
          <section className="panel">
            <h2>大模型（OpenAI 兼容 /chat/completions）</h2>
            <p className="hint">
              点击下列预设可一键填入 Base URL；API Key 仍须自行填写。豆包等区域与 Endpoint 以火山引擎控制台为准。
              豆包方舟：「模型名」须填在线推理里推理接入点的 Endpoint ID（形如 <code className="mono">ep-…</code>
              ），勿填控制台展示的模型名称（如 <code className="mono">Doubao-Seed-2.0-pro</code>），否则会报 404 / NotFound。
            </p>
            <div className="preset-row">
              {LLM_PRESETS.map((p) => (
                <button
                  key={p.baseUrl + p.label}
                  type="button"
                  className={
                    "btn secondary preset-chip" + (p.recommended ? " preset-chip--recommended" : "")
                  }
                  onClick={() => applyPreset(p.baseUrl, p.modelPlaceholder)}
                >
                  {p.label}
                  {p.recommended ? <span className="preset-badge">推荐</span> : null}
                </button>
              ))}
            </div>
            <label className="label">API Base URL</label>
            <input
              className="field"
              value={settings.llmBaseUrl}
              onChange={(e) => setSettings({ ...settings, llmBaseUrl: e.target.value })}
              placeholder="https://api.moonshot.cn/v1"
            />
            <label className="label">API Key</label>
            <input
              className="field"
              type="password"
              autoComplete="off"
              value={settings.llmApiKey}
              onChange={(e) => setSettings({ ...settings, llmApiKey: e.target.value })}
            />
            <label className="label">模型名</label>
            <input
              className="field"
              value={settings.llmModel}
              onChange={(e) => setSettings({ ...settings, llmModel: e.target.value })}
              placeholder="如 moonshot-v1-8k"
            />
            <h3 className="settings-subblock-title">Gemini 联网（Google Search）</h3>
            <p className="hint">勾选后主生成请求会附带 <code className="mono">web_search_options</code>（需 LiteLLM 等转发至 Gemini）。豆包/Kimi 请在网关 <code className="mono">drop_params</code> 或勿勾选。</p>
            <label className="label checkbox-label">
              <input
                type="checkbox"
                checked={settings.enableGeminiGoogleSearch}
                onChange={(e) =>
                  setSettings({ ...settings, enableGeminiGoogleSearch: e.target.checked })
                }
              />
              <span>
                Gemini 联网（Google Search）：在 API 请求中加入 <code className="mono">web_search_options</code>
                ，需 LiteLLM 等网关转发至 Gemini；豆包/Kimi 等请在网关开启 <code className="mono">drop_params</code> 或勿勾选
              </span>
            </label>
            <label className="label">联网检索上下文规模</label>
            <select
              className="field narrow"
              value={settings.geminiWebSearchContextSize}
              onChange={(e) =>
                setSettings({
                  ...settings,
                  geminiWebSearchContextSize: normalizeSearchContextSize(e.target.value),
                })
              }
              disabled={!settings.enableGeminiGoogleSearch}
            >
              <option value="low">low</option>
              <option value="medium">medium</option>
              <option value="high">high</option>
            </select>
            <p className="hint">
              直连 Google Gemini API 时请改用官方 <code className="mono">generateContent</code> +{" "}
              <code className="mono">google_search</code> 工具；本应用通过 OpenAI 兼容字段对接 LiteLLM。
            </p>
          </section>

          <section className="panel">
            <h2>个人简介（可选）</h2>
            <p className="hint">
              写一次即可：用于生成<strong>邮件标题</strong>中的身份要点（如姓名、院校、学历、方向）。建议只列要点（约十几行内），完整经历请用撰写页的 PDF
              附件；过长内容会在请求时截断以控制 API token 用量。
            </p>
            <textarea
              className="field tall"
              value={settings.resumeProfile}
              onChange={(e) => setSettings({ ...settings, resumeProfile: e.target.value })}
              placeholder={
                "示例：\n张三｜清华大学硕士｜计算机｜研究方向：分布式系统\n或按行：姓名 / 学校 / 学历 / 实习与项目要点…"
              }
              spellCheck={false}
            />
            {settings.resumeProfile.length > 0 && (
              <p className="hint">当前约 {settings.resumeProfile.length} 字；发往模型时超过约 8000 字会截断。</p>
            )}
          </section>

          <section className="panel">
            <h2>生成 Prompt（单次调用）</h2>
            <p className="hint">{PROMPT_LAYOUT_BLURB}</p>
            <p className="hint">
              后端仍期望模型输出可解析的 JSON（<code className="mono">subject</code>、<code className="mono">body</code>
              ）。下方输入框<strong>留空则完全不注入</strong>；推荐全文仅作复制参考，不会自动写入。
            </p>
            <details className="prompt-example">
              <summary>① 推荐 System 全文（复制到下方「System」框）</summary>
              <p className="hint">角色、JSON 契约、业务与实习细节标准、律所语体与「人味」约束等，放在 System。</p>
              <div className="row gap wrap">
                <button
                  type="button"
                  className="btn secondary"
                  onClick={() => copyPromptRef(REFERENCE_SYSTEM_PROMPT, "推荐 System")}
                >
                  复制推荐 System
                </button>
              </div>
              <pre className="example-block prompt-ref">{REFERENCE_SYSTEM_PROMPT}</pre>
            </details>
            <details className="prompt-example">
              <summary>② 推荐 User 末尾补充（复制到下方「User 消息末尾补充」框）</summary>
              <p className="hint">
                会拼在<strong>岗位描述、策略 Markdown 之后</strong>，适合放「最后阅读」的篇幅与结构硬性约束；与 System 冲突时以本段为准。
              </p>
              <div className="row gap wrap">
                <button
                  type="button"
                  className="btn secondary"
                  onClick={() => copyPromptRef(REFERENCE_USER_SUFFIX_PROMPT, "推荐 User 末尾")}
                >
                  复制推荐 User 末尾
                </button>
              </div>
              <pre className="example-block prompt-ref">{REFERENCE_USER_SUFFIX_PROMPT}</pre>
            </details>
            <details className="prompt-example">
              <summary>③ 示例输出（仅供参考，非默认）</summary>
              <p className="hint">{PROMPT_EXAMPLE_NOTE}</p>
              <p className="example-label">
                <span className="muted-tag">subject</span>
              </p>
              <pre className="example-block mono">{PROMPT_EXAMPLE_SUBJECT}</pre>
              <p className="example-label">
                <span className="muted-tag">body</span>
              </p>
              <pre className="example-block">{PROMPT_EXAMPLE_BODY}</pre>
            </details>
            <label className="label">System（可选）</label>
            <textarea
              className="field tall"
              value={settings.systemPrompt}
              onChange={(e) => setSettings({ ...settings, systemPrompt: e.target.value })}
              placeholder="留空：请求中不包含 system 消息"
              spellCheck={false}
            />
            <label className="label">User 消息末尾补充（可选）</label>
            <textarea
              className="field tall"
              value={settings.emailUserSuffixPrompt}
              onChange={(e) => setSettings({ ...settings, emailUserSuffixPrompt: e.target.value })}
              placeholder="拼在岗位描述与策略之后；留空则不追加"
              spellCheck={false}
            />
          </section>

          <section className="panel">
            <h2>发信 SMTP</h2>
            <p className="hint">
              Gmail 等请使用应用专用密码；企业邮箱以 IT 提供的 SMTP 为准。信息保存在本机用户目录下的配置文件，请勿共享屏幕时泄露。
            </p>
            <label className="label">SMTP 主机</label>
            <input
              className="field"
              value={settings.smtpHost}
              onChange={(e) => setSettings({ ...settings, smtpHost: e.target.value })}
              placeholder="smtp.gmail.com"
            />
            <label className="label">端口</label>
            <input
              className="field narrow"
              type="number"
              value={settings.smtpPort || 587}
              onChange={(e) => setSettings({ ...settings, smtpPort: Number(e.target.value) || 587 })}
            />
            <label className="label">SMTP 用户名</label>
            <input
              className="field"
              value={settings.smtpUsername}
              onChange={(e) => setSettings({ ...settings, smtpUsername: e.target.value })}
            />
            <label className="label">SMTP 密码 / 应用专用密码</label>
            <input
              className="field"
              type="password"
              autoComplete="off"
              value={settings.smtpPassword}
              onChange={(e) => setSettings({ ...settings, smtpPassword: e.target.value })}
            />
            <label className="label">发件人（含或不含显示名，如 Name &lt;a@b.com&gt;）</label>
            <input
              className="field"
              value={settings.fromEmail}
              onChange={(e) => setSettings({ ...settings, fromEmail: e.target.value })}
              placeholder={"Your Name <you@gmail.com>"}
            />
            <button type="button" className="btn primary" onClick={saveSettings} disabled={!!busy}>
              保存设置
            </button>
          </section>
        </main>
      )}
    </div>
  );
}

export default App;
