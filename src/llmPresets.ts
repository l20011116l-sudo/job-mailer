/** OpenAI 兼容 /chat/completions；模型名以各厂商控制台为准 */
export type LlmPreset = {
  label: string;
  baseUrl: string;
  /** 填入 Base URL 时若模型名为空则写入此占位，便于对照文档修改 */
  modelPlaceholder: string;
};

export const LLM_PRESETS: LlmPreset[] = [
  {
    label: "Kimi Moonshot",
    baseUrl: "https://api.moonshot.cn/v1",
    modelPlaceholder: "moonshot-v1-8k",
  },
  {
    label: "豆包方舟（北京）",
    baseUrl: "https://ark.cn-beijing.volces.com/api/v3",
    modelPlaceholder: "控制台 Endpoint ID（ep-…）",
  },
  {
    label: "Gemini OpenAI 兼容",
    baseUrl: "https://generativelanguage.googleapis.com/v1beta/openai",
    modelPlaceholder: "gemini-2.0-flash",
  },
];
