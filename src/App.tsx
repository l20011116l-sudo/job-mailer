import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { LLM_PRESETS } from "./llmPresets";

type StoredSettings = {
  llmBaseUrl: string;
  llmApiKey: string;
  llmModel: string;
  /** 第二轮：邮件 JSON 的 system；空则后端填入内置短契约 */
  systemPrompt: string;
  /** 第一轮：JD 对齐专用 system；空则用内置 JD 对齐 prompt */
  jdAlignmentSystemPrompt: string;
  /** 第二轮：拼在 user 消息末尾的硬性约束；空则用内置 USER_GENERATION_TAIL */
  emailUserSuffixPrompt: string;
  resumeProfile: string;
  enableJdAlignmentDigest: boolean;
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
  jdAlignmentSystemPrompt: "",
  emailUserSuffixPrompt: "",
  resumeProfile: "",
  enableJdAlignmentDigest: true,
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
          enableJdAlignmentDigest: s.enableJdAlignmentDigest ?? true,
          jdAlignmentSystemPrompt: s.jdAlignmentSystemPrompt ?? "",
          emailUserSuffixPrompt: s.emailUserSuffixPrompt ?? "",
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
        enableJdAlignmentDigest: settings.enableJdAlignmentDigest,
        jdAlignmentSystemPrompt: settings.jdAlignmentSystemPrompt,
        emailUserSuffixPrompt: settings.emailUserSuffixPrompt,
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
              <p className="hint">策略已载入，约 {strategyMd.length} 字。正文「可核对事实」的上限主要由策略文件决定，建议写入项目名、成果、工具等可展开条目。</p>
            ) : (
              <p className="hint">
                未选策略文件时：仍会用「JD 岗位事实预生成」中的业务白话 / JD 事实 / 岗位侧要求（若已开启）与系统内置准备模块支撑首段；第二段不得虚构经历。若有可展开的项目或实习，可载入策略 Markdown 提高上限。
              </p>
            )}
          </section>

          <section className="panel">
            <h2>生成与审核</h2>
            {settings.enableJdAlignmentDigest ? (
              <p className="hint">
                已开启「JD 岗位事实预生成」：第一次调用按 JD 生成三段备忘录——「业务白话」「JD事实」「岗位侧要求」；第二次调用再写完整正文（约四五百字），与系统「内置准备模块」对齐。可在「设置」中关闭以对比效果。
              </p>
            ) : (
              <p className="hint">
                已关闭「JD 岗位事实预生成」：仅调用一次模型生成全文（仍以约四五百字为目标），更省耗时与 token。可在「设置」中重新开启。
              </p>
            )}
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
            <p className="hint">点击下列预设可一键填入 Base URL；API Key 仍须自行填写。豆包等区域与 Endpoint 以火山引擎控制台为准。</p>
            <div className="preset-row">
              {LLM_PRESETS.map((p) => (
                <button
                  key={p.label}
                  type="button"
                  className="btn secondary preset-chip"
                  onClick={() => applyPreset(p.baseUrl, p.modelPlaceholder)}
                >
                  {p.label}
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
            <label className="label checkbox-label">
              <input
                type="checkbox"
                checked={settings.enableJdAlignmentDigest}
                onChange={(e) =>
                  setSettings({ ...settings, enableJdAlignmentDigest: e.target.checked })
                }
              />
              <span>
                生成邮件前先根据 JD 生成岗位侧事实备忘录（多一次 API，首段更易呈现「调研感」；正文目标约四五百字）
              </span>
            </label>
            <p className="hint">关闭后仅单次调用生成全文，便于对比效果与节省用量。修改后请点下方「保存设置」。</p>
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
            <h2>生成 Prompt（两轮分离）</h2>
            <p className="hint">
              <strong>第一轮</strong>仅做 JD→备忘录（第三方视角），<strong>第二轮</strong>才写求职邮件 JSON。留空则分别使用应用内置模板，避免把「写邮件」和「整理 JD」混在一条指令里。
            </p>
            <label className="label">第一轮：JD 对齐（system）</label>
            <textarea
              className="field tall"
              value={settings.jdAlignmentSystemPrompt}
              onChange={(e) => setSettings({ ...settings, jdAlignmentSystemPrompt: e.target.value })}
              placeholder="留空：使用内置「岗位信息整理」说明（第三方备忘录，非求职信）"
              spellCheck={false}
            />
            <label className="label">第二轮：邮件生成（system）</label>
            <textarea
              className="field tall"
              value={settings.systemPrompt}
              onChange={(e) => setSettings({ ...settings, systemPrompt: e.target.value })}
              placeholder="留空：由应用填入内置短契约（JSON 字段、禁编造等）"
              spellCheck={false}
            />
            <label className="label">第二轮：user 消息末尾硬性约束（可选）</label>
            <textarea
              className="field tall"
              value={settings.emailUserSuffixPrompt}
              onChange={(e) => setSettings({ ...settings, emailUserSuffixPrompt: e.target.value })}
              placeholder="留空：使用内置「硬性约束」全文（反套话、结构、结尾等），拼在岗位描述与策略之后"
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
