# LiteLLM 代理：豆包 / Kimi + job-mailer

## 1. 为什么前面挂 LiteLLM

- job-mailer 只发 **OpenAI 兼容** 的 `POST …/chat/completions`。
- 豆包（火山）、Kimi（Moonshot）各自有 **Base URL、密钥、模型名**；用 LiteLLM 可以在本机把多家合成 **一个** `http://127.0.0.1:4000/v1`，师弟师妹只改「模型名」即可切换。

## 2. 安装与启动

```bash
pip install "litellm[proxy]"   # 或按你环境使用 uv/poetry
export MOONSHOT_API_KEY=你的key
export VOLCENGINE_API_KEY=你的key   # 豆包 / 方舟
cd job-mailer
litellm --config litellm.config.example.yaml
```

## 3. job-mailer 里怎么填

| 设置项 | 示例值 |
|--------|--------|
| API Base URL | `http://127.0.0.1:4000/v1` |
| API Key | 与 `litellm.config.example.yaml` 里 `general_settings.master_key` 一致（默认 `sk-job-mailer-local`） |
| 模型 | 配置里的 `model_name`，如 `kimi-k2-5`、`doubao-chat` |

程序会请求 `{Base}/chat/completions`，因此 **Base 必须带 `/v1`**，末尾不要多斜杠。

## 4. Job Mailer 内「Gemini 联网」开关

应用设置中可勾选 **Gemini 联网（Google Search）**：生成时会在 `POST /v1/chat/completions` 的 JSON 里附加 `web_search_options`（含 `search_context_size`）。LiteLLM 会将其映射到 **Gemini 的 Google Search Grounding**。建议勾选 **仅第一轮 JD 对齐使用联网**，以控制检索次数与费用。

## 5. 「Deep Search / 联网」与豆包、Kimi

**方案 A（本仓库示例里的 `web_search_options`）**  
LiteLLM 文档里 **`web_search_options` 与 Gemini、部分 OpenAI 搜索型号等绑定**；豆包、Kimi **未必**走同一套参数。示例里对 Kimi/豆包默认 **注释掉** `<<: *web_search_defaults`，并开启 `drop_params: true`，避免不支持的字段导致报错。若你使用的 LiteLLM 版本与文档已支持 Moonshot/Volcengine 的联网参数，可再按需取消注释。

**方案 B（与厂商无关的搜索）**  
使用 LiteLLM 的 **Web Search Interception**（Perplexity、Tavily 等），在代理侧配置 `search_tools` 与 `websearch_interception` 回调。注意：常见用法是客户端或模型**发起搜索类 tool**；job-mailer 当前请求体**不带 tools**，若要完全自动化，需后续在应用或代理层增加「请求前注入」类能力，或查阅你所用 LiteLLM 版本是否支持默认注入。

**方案 C（最省事）**  
在 **火山 / Moonshot 控制台** 直接选用**已带联网能力**的模型或 Endpoint，不依赖 `web_search_options`。

## 6. Kimi 国内站

推荐使用国内 API：`https://api.moonshot.cn/v1`。可在启动前执行：

```bash
export MOONSHOT_API_BASE=https://api.moonshot.cn/v1
```

或在 `litellm.config.example.yaml` 末尾取消 `environment_variables` 注释。

## 7. 豆包 Endpoint

在火山方舟复制 **Endpoint ID**，把配置里的 `volcengine/ep-your-endpoint-id` 换成真实 ID（形如 `ep-2025xxxx-xxxxx`）。
