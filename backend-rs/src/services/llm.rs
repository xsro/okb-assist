//! LLM / Embedding 服务适配器。
//!
//! 支持两种协议：
//! - `ollama`：Ollama 原生协议（`/api/generate`、`/api/embed`、`/api/tags`）
//! - `openai`：OpenAI 兼容协议（`/v1/chat/completions`、`/v1/embeddings`、`/v1/models`）
//!
//! 二者的请求/响应结构并不相同（例如生成结果在 `response` 还是
//! `choices[0].message.content`、向量在 `embeddings` 还是 `data[].embedding`），
//! 因此由本模块内的 `Protocol` 分支统一适配。
//!
//! LLM 配置与 Embedding 配置相互独立：同一个客户端类型用不同配置构造即可。

use serde_json::{json, Value};

/// 元数据抽取的默认提示词模板。
///
/// 占位符：
/// - `{metadata}`：已有的元数据（JSON，用于纠错；无则为“（空）”）
/// - `{content}`：Markdown 正文（按 `max_input_chars` 截断）
///
/// 可在 `config.json` 的 `llm_extract.prompt` 中整体替换。
pub const DEFAULT_EXTRACT_PROMPT: &str = r#"你是学术文献元数据校对助手。下面给出「已有元数据」与「Markdown 正文」，
请以正文为唯一事实来源，输出校对后的完整元数据。

校对规则（逐字段执行）：
1. 正文明确定义了该字段 → 以正文为准，即使与已有元数据不同也要覆盖。
2. 已有值明显不对（与正文矛盾、格式错误如年份不是数字、DOI 不合法、标题里混进期刊名或模板文字、
   作者里混进单位、乱码等）→ 正文中能找到正确取值则改正；**正文里也找不到就留空**
   （字符串填 ""，列表填 []，year 填 null）。禁止猜测、禁止沿用错误值凑数。
3. 已有值正确、正文未涉及该字段 → 原样保留。
4. 已有值为空 → 尽量从正文补全；正文确实没有该信息时保持为空。
5. 严禁编造。宁可留空，也不要凭常识、文件名或模板臆测。

语言规则：
- language 使用 ISO 639-1 代码（en/zh/ja/fr/ru/de/ko/es/pt/ar 等），按正文判断。
- language 不是 "en" 时，同时给出 *_en 字段（英文译名/罗马化）；language 为 "en" 时 *_en 字段留空。

type 必须是 Zotero 类型之一：
journalArticle|book|conferencePaper|thesis|report|preprint|bookSection|webpage|document|patent|review|manuscript|presentation
（期刊论文用 journalArticle，会议论文用 conferencePaper）

只输出一个 JSON 对象，不要解释文字，不要 Markdown 代码块。结构如下：
{"language": "", "type": "", "title": "", "title_en": "", "year": null, "authors": [], "authors_en": [],
 "abstract": "", "abstract_en": "", "doi": "", "source": "", "journal": "", "journal_en": "",
 "keywords": [], "keywords_en": [], "category": ""}

已有元数据（可能为空或包含错误）：
{metadata}

Markdown 正文（可能被截断）：
{content}"#;

/// 元数据抽取参数。
#[derive(Debug, Clone)]
pub struct ExtractOptions {
    /// 自定义提示词模板（支持 `{content}` / `{metadata}`；空 = [`DEFAULT_EXTRACT_PROMPT`]）
    pub prompt_template: String,
    /// 已有元数据（用于纠错；`None` 表示没有）
    pub existing: Option<Value>,
    /// 送入模型的 Markdown 最大字符数
    pub max_input_chars: usize,
    /// 生成的最大 token 数（摘要较长时需要更大）
    pub max_tokens: u32,
}

impl Default for ExtractOptions {
    fn default() -> Self {
        Self {
            prompt_template: String::new(),
            existing: None,
            max_input_chars: 6000,
            max_tokens: 2048,
        }
    }
}

/// 截断到指定字符数（按字符而非字节，避免切坏中文）
fn truncate_chars(text: &str, max_chars: usize) -> String {
    if max_chars == 0 || text.chars().count() <= max_chars {
        return text.to_string();
    }
    text.chars().take(max_chars).collect()
}

/// 构造元数据抽取提示词。
///
/// - `template` 为空时使用 [`DEFAULT_EXTRACT_PROMPT`]；
/// - `{metadata}` 替换为已有元数据的 JSON（无则“（空）”），`{content}` 替换为截断后的正文；
/// - 自定义模板漏写 `{content}` 时，自动在末尾补上正文，避免模型看不到内容。
pub fn build_extract_prompt(
    template: &str,
    existing: Option<&Value>,
    content: &str,
    max_input_chars: usize,
) -> String {
    let template = if template.trim().is_empty() {
        DEFAULT_EXTRACT_PROMPT
    } else {
        template
    };

    let metadata_text = match existing {
        Some(v) if !v.is_null() && v.as_object().map(|o| !o.is_empty()).unwrap_or(false) => {
            serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
        }
        _ => "（空）".to_string(),
    };

    let body = truncate_chars(content, max_input_chars);

    // 先替换 metadata（避免正文里出现 {metadata} 时被二次替换），正文最后替换
    let prompt = template.replace("{metadata}", &metadata_text);
    if prompt.contains("{content}") {
        prompt.replace("{content}", &body)
    } else {
        format!("{}\n\nMarkdown 正文（可能被截断）：\n{}", prompt, body)
    }
}

/// LLM 协议类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// Ollama 原生协议
    Ollama,
    /// OpenAI 兼容协议
    OpenAi,
}

impl Protocol {
    /// 解析配置中的 protocol 字段，无法识别时回退到 Ollama 原生协议
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "openai" | "open-ai" | "open_ai" | "v1" => Protocol::OpenAi,
            _ => Protocol::Ollama,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Protocol::Ollama => "ollama",
            Protocol::OpenAi => "openai",
        }
    }

    /// 协议对所有兼容实现均有效的展示名
    fn label(&self) -> &'static str {
        match self {
            Protocol::Ollama => "Ollama",
            Protocol::OpenAi => "OpenAI",
        }
    }
}

/// 工具调用方式
///
/// - `Auto`：先按原生 function calling 发；若服务商根本不支持（报错含 tool/function）
///   或模型该轮不动手（回答里表示自己无法访问文献库），自动降级为提示词协议
/// - `Native`：只用原生 `tools`（Ollama / OpenAI 兼容函数调用）
/// - `Prompt`：只用提示词协议（模型输出 `{"tool_calls":[...]}` JSON），
///   适用于不支持 function calling 的模型（如部分国内网关/思考型模型）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolMode {
    Auto,
    Native,
    Prompt,
}

impl ToolMode {
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "native" | "function" | "tools" => ToolMode::Native,
            "prompt" | "text" | "json" => ToolMode::Prompt,
            _ => ToolMode::Auto,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            ToolMode::Auto => "auto",
            ToolMode::Native => "native",
            ToolMode::Prompt => "prompt",
        }
    }
}

/// 对话消息（OpenAI 风格的角色 + 内容，Ollama 原生协议也使用同一结构）
///
/// 带工具的对话需要三种附加字段：
/// - `tool_calls`：assistant 消息发起的工具调用
/// - `tool_call_id` / `tool_name`：tool 角色消息回传的结果归属（OpenAI / Ollama 各用一个）
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ChatMessage {
    /// "system" / "user" / "assistant" / "tool"
    pub role: String,
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    /// OpenAI 协议：工具结果消息需带 `tool_call_id`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Ollama 原生协议：工具结果消息需带 `tool_name`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
}

/// 模型请求的一次工具调用
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolCall {
    /// 调用 id：OpenAI 协议必须回传；Ollama 通常不返回，缺失时由本模块生成
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// 调用参数（统一为 JSON 对象；OpenAI 返回的 JSON 字符串在解析时已展开）
    #[serde(default)]
    pub arguments: Value,
}

/// 一次对话生成的结果（可能包含工具调用请求）
#[derive(Debug, Clone, Default)]
pub struct ChatReply {
    /// 文本回答（发起工具调用时通常为空）
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    /// 结束原因（OpenAI 的 `finish_reason` / Ollama 的 `done_reason`），
    /// 用于诊断“content 为空”是不是被 max_tokens 截断
    pub finish_reason: Option<String>,
}

impl ChatMessage {
    pub fn new(role: &str, content: &str) -> Self {
        Self {
            role: role.to_string(),
            content: content.to_string(),
            ..Default::default()
        }
    }

    /// assistant 消息：携带工具调用请求
    pub fn assistant_tool_calls(content: &str, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.to_string(),
            tool_calls,
            ..Default::default()
        }
    }

    /// tool 消息：回传工具执行结果
    pub fn tool_result(call: &ToolCall, content: &str) -> Self {
        Self {
            role: "tool".to_string(),
            content: content.to_string(),
            tool_calls: Vec::new(),
            tool_call_id: Some(call.id.clone()),
            tool_name: Some(call.name.clone()),
        }
    }

    /// 是否为 LLM 支持的对话角色
    pub fn is_valid_role(&self) -> bool {
        matches!(self.role.as_str(), "system" | "user" | "assistant" | "tool")
    }

    /// 是否携带实质内容（不能因为 content 为空就丢弃带工具调用的 assistant 消息）
    pub fn is_meaningful(&self) -> bool {
        !self.content.trim().is_empty() || !self.tool_calls.is_empty() || self.role == "tool"
    }
}

/// 把工具参数统一成 JSON 对象
///
/// - OpenAI 协议返回的是 JSON 字符串，需要展开
/// - Ollama 返回的是对象，直接使用
/// - 解析失败时退化为空对象（不因参数不合法而放弃整轮对话）
fn normalize_arguments(raw: &Value) -> Value {
    match raw {
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty() {
                json!({})
            } else {
                serde_json::from_str::<Value>(s).unwrap_or_else(|_| json!({}))
            }
        }
        Value::Object(_) => raw.clone(),
        _ => json!({}),
    }
}

pub struct LlmClient {
    protocol: Protocol,
    base_url: String,
    key: Option<String>,
    model: String,
    http: reqwest::Client,
}

impl LlmClient {
    pub fn new(protocol: Protocol, base_url: &str, key: Option<&str>, model: &str) -> Self {
        Self {
            protocol,
            base_url: base_url.trim().trim_end_matches('/').to_string(),
            // 空 key 视作未配置（Ollama 原生协议通常无鉴权）
            key: key
                .map(str::trim)
                .filter(|k| !k.is_empty())
                .map(String::from),
            model: model.to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// OpenAI 协议下自动补全 `/v1` 前缀，Ollama 原生协议使用原始 base_url
    fn base(&self) -> String {
        match self.protocol {
            Protocol::OpenAi if !self.base_url.ends_with("/v1") => {
                format!("{}/v1", self.base_url)
            }
            _ => self.base_url.clone(),
        }
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        let req = self.http.post(format!("{}{}", self.base(), path));
        match self.key {
            Some(ref k) => req.bearer_auth(k),
            None => req,
        }
    }

    /// 发送请求并解析 JSON 响应，失败时提取两种协议各自的错误字段
    async fn send_json(&self, req: reqwest::RequestBuilder) -> anyhow::Result<Value> {
        let resp = req.send().await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            anyhow::bail!(
                "{} 返回错误状态 {}: {}",
                self.protocol.label(),
                status,
                describe_error(&text)
            );
        }
        serde_json::from_str(&text).map_err(|e| {
            anyhow::anyhow!(
                "{} 响应不是合法 JSON: {}（原文: {}）",
                self.protocol.label(),
                e,
                text.chars().take(200).collect::<String>()
            )
        })
    }

    /// 生成文本（元数据抽取等场景）
    pub async fn generate(
        &self,
        prompt: &str,
        temperature: f64,
        max_tokens: u32,
    ) -> anyhow::Result<String> {
        let (path, body) = match self.protocol {
            // Ollama 原生：参数嵌在 options 中，num_predict 等价于 max_tokens
            Protocol::Ollama => (
                "/api/generate",
                json!({
                    "model": self.model,
                    "prompt": prompt,
                    "stream": false,
                    "options": {
                        "temperature": temperature,
                        "num_predict": max_tokens,
                    }
                }),
            ),
            // OpenAI 兼容：参数平铺在顶层
            Protocol::OpenAi => (
                "/chat/completions",
                json!({
                    "model": self.model,
                    "messages": [{"role": "user", "content": prompt}],
                    "temperature": temperature,
                    "max_tokens": max_tokens,
                    "stream": false
                }),
            ),
        };

        let data = self.send_json(self.post(path).json(&body)).await?;

        Ok(self.parse_generate_text(&data))
    }

    /// 从生成响应中取出文本
    ///
    /// - Ollama 原生：`{"response": "..."}`
    /// - OpenAI 兼容：`{"choices": [{"message": {"content": "..."}}]}`
    fn parse_generate_text(&self, data: &Value) -> String {
        match self.protocol {
            Protocol::Ollama => data["response"].as_str().unwrap_or("").to_string(),
            Protocol::OpenAi => self.parse_chat_text(data),
        }
    }

    /// 从对话响应中取出文本
    ///
    /// - Ollama 原生：`{"message": {"content": "..."}}`
    /// - OpenAI 兼容：`{"choices": [{"message": {"content": "..."}}]}`
    fn parse_chat_text(&self, data: &Value) -> String {
        match self.protocol {
            Protocol::Ollama => data["message"]["content"].as_str().unwrap_or("").to_string(),
            Protocol::OpenAi => data["choices"]
                .as_array()
                .and_then(|choices| choices.first())
                .and_then(|c| c["message"]["content"].as_str())
                .unwrap_or("")
                .to_string(),
        }
    }

    /// 多轮对话生成（非流式，可携带工具）。
    ///
    /// `tools` 为 MCP 风格的工具定义（`{name, description, inputSchema}`），
    /// 内部转换为两种协议的 `tools[].function.{name, description, parameters}`。
    /// 返回文本回答与（可能的）工具调用请求。
    pub async fn chat_with_tools(
        &self,
        messages: &[ChatMessage],
        tools: &[Value],
        temperature: f64,
        max_tokens: u32,
    ) -> anyhow::Result<ChatReply> {
        let msgs = self.messages_to_value(messages);

        let (path, mut body) = match self.protocol {
            // Ollama 原生：参数嵌在 options 中，num_predict 等价于 max_tokens
            Protocol::Ollama => (
                "/api/chat",
                json!({
                    "model": self.model,
                    "messages": msgs,
                    "stream": false,
                    "options": {
                        "temperature": temperature,
                        "num_predict": max_tokens,
                    }
                }),
            ),
            // OpenAI 兼容：参数平铺在顶层
            Protocol::OpenAi => (
                "/chat/completions",
                json!({
                    "model": self.model,
                    "messages": msgs,
                    "temperature": temperature,
                    "max_tokens": max_tokens,
                    "stream": false
                }),
            ),
        };

        // 两种协议的工具定义结构一致（MCP 的 inputSchema → function.parameters）
        if !tools.is_empty() {
            body["tools"] = Value::Array(self.tool_specs(tools));
        }

        let data = self.send_json(self.post(path).json(&body)).await?;

        Ok(self.parse_chat_reply(&data))
    }

    /// MCP 工具定义 → 两种协议的 `tools` 数组
    fn tool_specs(&self, tools: &[Value]) -> Vec<Value> {
        tools
            .iter()
            .filter_map(|t| {
                let name = t["name"].as_str()?;
                Some(json!({
                    "type": "function",
                    "function": {
                        "name": name,
                        "description": t["description"].as_str().unwrap_or(""),
                        "parameters": t
                            .get("inputSchema")
                            .cloned()
                            .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
                    }
                }))
            })
            .collect()
    }

    /// 消息列表 → 协议专属的 `messages` 数组
    ///
    /// 带工具的会话里，assistant 的工具调用与 tool 结果消息结构不同：
    /// - Ollama：assistant `{tool_calls:[{function:{name,arguments}}]}`，tool 消息带 `tool_name`
    /// - OpenAI：assistant `{tool_calls:[{id,type,function:{name,arguments(json 字符串)}}]}`，
    ///   tool 消息带 `tool_call_id`
    fn messages_to_value(&self, messages: &[ChatMessage]) -> Vec<Value> {
        messages
            .iter()
            .map(|m| match self.protocol {
                Protocol::Ollama => {
                    let mut v = json!({"role": m.role, "content": m.content});
                    if !m.tool_calls.is_empty() {
                        v["tool_calls"] = Value::Array(
                            m.tool_calls
                                .iter()
                                .map(|c| {
                                    json!({
                                        "function": {
                                            "name": c.name,
                                            "arguments": c.arguments,
                                        }
                                    })
                                })
                                .collect(),
                        );
                    }
                    if m.role == "tool" {
                        if let Some(name) = m.tool_name.as_deref() {
                            v["tool_name"] = json!(name);
                        }
                    }
                    v
                }
                Protocol::OpenAi => {
                    let mut v = json!({"role": m.role, "content": m.content});
                    if !m.tool_calls.is_empty() {
                        v["tool_calls"] = Value::Array(
                            m.tool_calls
                                .iter()
                                .map(|c| {
                                    json!({
                                        "id": c.id,
                                        "type": "function",
                                        "function": {
                                            "name": c.name,
                                            // OpenAI 要求 arguments 是 JSON 字符串
                                            "arguments": c.arguments.to_string(),
                                        }
                                    })
                                })
                                .collect(),
                        );
                    }
                    if m.role == "tool" {
                        if let Some(id) = m.tool_call_id.as_deref() {
                            v["tool_call_id"] = json!(id);
                        }
                    }
                    v
                }
            })
            .collect()
    }

    /// 解析对话响应：文本 + 工具调用请求
    fn parse_chat_reply(&self, data: &Value) -> ChatReply {
        ChatReply {
            content: self.parse_chat_text(data),
            tool_calls: self.parse_tool_calls(data),
            finish_reason: self.parse_finish_reason(data),
        }
    }

    /// 结束原因（Ollama `done_reason` / OpenAI `choices[0].finish_reason`）
    fn parse_finish_reason(&self, data: &Value) -> Option<String> {
        match self.protocol {
            Protocol::Ollama => data["done_reason"].as_str().map(String::from),
            Protocol::OpenAi => data["choices"]
                .as_array()
                .and_then(|choices| choices.first())
                .and_then(|c| c["finish_reason"].as_str())
                .map(String::from),
        }
    }

    /// 取出响应中的工具调用请求（Ollama 的 `message` / OpenAI 的 `choices[0].message`）
    fn parse_tool_calls(&self, data: &Value) -> Vec<ToolCall> {
        let message = match self.protocol {
            Protocol::Ollama => data.get("message"),
            Protocol::OpenAi => data["choices"]
                .as_array()
                .and_then(|choices| choices.first())
                .and_then(|c| c.get("message")),
        };
        let Some(items) = message.and_then(|m| m["tool_calls"].as_array()) else {
            return Vec::new();
        };

        items
            .iter()
            .enumerate()
            .filter_map(|(i, tc)| {
                let name = tc["function"]["name"].as_str().unwrap_or("").to_string();
                if name.is_empty() {
                    return None;
                }
                Some(ToolCall {
                    // Ollama 多数版本不返回 id，补一个以便回传 tool 结果
                    id: tc["id"]
                        .as_str()
                        .map(String::from)
                        .unwrap_or_else(|| format!("call_{}", i + 1)),
                    name,
                    arguments: normalize_arguments(&tc["function"]["arguments"]),
                })
            })
            .collect()
    }

    /// 从 Markdown 内容提取元数据。
    ///
    /// `options.prompt_template` 为空时使用内置默认提示词；`options.existing`
    /// 为已有元数据，会作为纠错依据一并送入模型。返回模型输出的原始 JSON
    /// （未与已有元数据合并，合并由调用方按 `overwrite` 策略完成）。
    pub async fn extract_metadata(
        &self,
        markdown_content: &str,
        options: &ExtractOptions,
    ) -> anyhow::Result<Value> {
        let prompt = build_extract_prompt(
            &options.prompt_template,
            options.existing.as_ref(),
            markdown_content,
            options.max_input_chars,
        );

        let response_text = self
            .generate(&prompt, 0.1, options.max_tokens.max(256))
            .await?;
        if response_text.trim().is_empty() {
            return Ok(empty_metadata());
        }

        Ok(parse_json_from_text(&response_text))
    }

    /// 获取单条文本的 embedding
    pub async fn get_embedding(&self, text: &str) -> anyhow::Result<Vec<f64>> {
        let mut embeddings = self.get_embeddings_batch(&[text.to_string()]).await?;
        if embeddings.is_empty() {
            anyhow::bail!("embedding 响应为空");
        }
        Ok(embeddings.remove(0))
    }

    /// 批量获取 embedding
    pub async fn get_embeddings_batch(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f64>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let (path, body) = match self.protocol {
            Protocol::Ollama => (
                "/api/embed",
                json!({"model": self.model, "input": texts}),
            ),
            Protocol::OpenAi => (
                "/embeddings",
                json!({"model": self.model, "input": texts}),
            ),
        };

        let data = self.send_json(self.post(path).json(&body)).await?;

        self.parse_embeddings(&data)
    }

    /// 从 embedding 响应中取出向量列表
    ///
    /// - Ollama 原生：`{"embeddings": [[...], [...]]}`（元素是裸数组）
    /// - OpenAI 兼容：`{"data": [{"index": 0, "embedding": [...]}]}`
    ///   （不保证顺序与入参一致，需按 `index` 排序）
    fn parse_embeddings(&self, data: &Value) -> anyhow::Result<Vec<Vec<f64>>> {
        match self.protocol {
            Protocol::Ollama => {
                let items = data["embeddings"]
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("响应缺少 embeddings 字段"))?;
                items.iter().map(parse_vec).collect()
            }
            Protocol::OpenAi => {
                let items = data["data"]
                    .as_array()
                    .ok_or_else(|| anyhow::anyhow!("响应缺少 data 字段"))?;
                let mut indexed: Vec<(i64, Vec<f64>)> = items
                    .iter()
                    .enumerate()
                    .map(|(position, item)| {
                        let idx = item["index"].as_i64().unwrap_or(position as i64);
                        let raw = item
                            .get("embedding")
                            .ok_or_else(|| anyhow::anyhow!("响应条目缺少 embedding 字段"))?;
                        Ok((idx, parse_vec(raw)?))
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                indexed.sort_by_key(|(idx, _)| *idx);
                Ok(indexed.into_iter().map(|(_, v)| v).collect())
            }
        }
    }

    /// 模型列表端点完整 URL
    fn models_url(&self) -> String {
        let path = match self.protocol {
            Protocol::Ollama => "/api/tags",
            Protocol::OpenAi => "/models",
        };
        format!("{}{}", self.base(), path)
    }

    /// 解析模型列表响应（Ollama 为 `models[].name`，OpenAI 为 `data[].id`）
    fn parse_model_list(&self, data: &Value) -> Vec<String> {
        let (list_key, name_key) = match self.protocol {
            Protocol::Ollama => ("models", "name"),
            Protocol::OpenAi => ("data", "id"),
        };
        data[list_key]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m[name_key].as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// 健康探测：请求模型列表端点，返回可用模型名。
    ///
    /// 与 [`Self::list_models`] 的区别在于不把“可达但报错”与“无法连接”混为一谈。
    pub async fn probe(&self, timeout_secs: u64) -> Result<Vec<String>, ProbeError> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build()
            .unwrap_or_default();
        let mut req = client.get(self.models_url());
        if let Some(key) = self.key.as_deref() {
            req = req.bearer_auth(key);
        }
        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                let models = match resp.json::<Value>().await {
                    Ok(data) => self.parse_model_list(&data),
                    Err(_) => Vec::new(),
                };
                Ok(models)
            }
            Ok(resp) => Err(ProbeError::Http(resp.status().as_u16())),
            Err(e) => Err(ProbeError::Unreachable(e.to_string())),
        }
    }
}

/// 健康探测失败原因
#[derive(Debug, Clone)]
pub enum ProbeError {
    /// 服务可达但返回了非 2xx 状态码
    Http(u16),
    /// 无法建立连接
    Unreachable(String),
}

impl ProbeError {
    /// 前端状态机使用的状态值与描述文本
    pub fn status_and_detail(&self) -> (&'static str, String) {
        match self {
            ProbeError::Http(code) => ("error", format!("HTTP {}", code)),
            ProbeError::Unreachable(msg) => ("disconnected", msg.clone()),
        }
    }
}

    /// 提示词工具协议：把工具清单与调用格式写入 system 提示词
///
/// 用于不支持原生 function calling 的模型（它们不会返回 `tool_calls`，
/// 只能按提示词约定输出 JSON）。
pub fn text_tool_prompt(tools: &[Value]) -> String {
    let schemas: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t["name"],
                "description": t["description"],
                "parameters": t
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
            })
        })
        .collect();
    let list = serde_json::to_string_pretty(&schemas).unwrap_or_else(|_| "[]".to_string());

    format!(
        "你可以调用下列工具查询本地文献库（均为只读查询）。工具清单（JSON Schema）：\n{}\n\n\
调用格式：需要调用工具时，**只输出一行 JSON**，不要输出其他文字：\n\
{{\"tool_calls\":[{{\"name\":\"工具名\",\"arguments\":{{\"参数名\":值}}}}]}}\n\
可以一次请求多个调用；参数为空时写 {{}}。\n\
工具执行结果会以【工具结果】的形式返回给你。信息足够后，请用自然语言直接作答，不要再输出 JSON。",
        list
    )
}

/// 从模型文本中解析提示词工具协议的调用请求
///
/// 容忍以下形态：
/// - `{"tool_calls":[{"name":"a","arguments":{}}]}`（推荐格式，可在代码块/噪声文本中）
/// - `{"tool":"a","arguments":{}}` / `{"name":"a","arguments":{}}`（单个调用）
/// - `arguments` 为 JSON 字符串（部分模型会这么输出）
pub fn parse_text_tool_calls(text: &str) -> Vec<ToolCall> {
    let mut candidates = vec![text.trim().to_string()];
    candidates.extend(json_object_candidates(text));

    for candidate in candidates {
        let cleaned = fix_trailing_commas(&candidate);
        let Ok(value) = serde_json::from_str::<Value>(&cleaned) else {
            continue;
        };
        let calls = text_tool_calls_from_value(&value);
        if !calls.is_empty() {
            return calls;
        }
    }
    Vec::new()
}

/// 把一个人工构造/模型输出的 JSON 值转成工具调用列表
fn text_tool_calls_from_value(value: &Value) -> Vec<ToolCall> {
    let mut calls = Vec::new();

    // 推荐格式：tool_calls 数组
    if let Some(items) = value["tool_calls"].as_array() {
        for (i, item) in items.iter().enumerate() {
            let name = item["name"]
                .as_str()
                .or_else(|| item["function"]["name"].as_str())
                .unwrap_or("")
                .trim();
            if name.is_empty() {
                continue;
            }
            let raw = item
                .get("arguments")
                .or_else(|| item.get("args"))
                .or_else(|| item.get("parameters"))
                .or_else(|| item["function"].get("arguments"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            calls.push(ToolCall {
                id: format!("text_call_{}", i + 1),
                name: name.to_string(),
                arguments: normalize_arguments(&raw),
            });
        }
        if !calls.is_empty() {
            return calls;
        }
    }

    // 单调用格式：{"tool":"x",...} 或 {"name":"x","arguments":{...}}
    let solo = value["tool"].as_str().or_else(|| value["name"].as_str()).unwrap_or("");
    if !solo.trim().is_empty() {
        let raw = value
            .get("arguments")
            .or_else(|| value.get("args"))
            .or_else(|| value.get("parameters"))
            .cloned()
            .unwrap_or_else(|| json!({}));
        calls.push(ToolCall {
            id: "text_call_1".to_string(),
            name: solo.trim().to_string(),
            arguments: normalize_arguments(&raw),
        });
    }

    calls
}

/// 列出文本中所有顶层平衡的 `{...}` 片段（字符串内部的括号被正确跳过）
fn json_object_candidates(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '{' {
            i += 1;
            continue;
        }
        let mut depth = 0usize;
        let mut in_str = false;
        let mut escaped = false;
        let mut j = i;
        while j < chars.len() {
            let c = chars[j];
            if in_str {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    in_str = false;
                }
            } else {
                match c {
                    '"' => in_str = true,
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            out.push(chars[i..=j].iter().collect());
                            break;
                        }
                    }
                    _ => {}
                }
            }
            j += 1;
        }
        i = if j > i { j + 1 } else { i + 1 };
    }
    out
}

/// 容忍模型输出中的尾随逗号
fn fix_trailing_commas(text: &str) -> String {
    let text = text.trim();
    let text = text.strip_prefix("```json").unwrap_or(text);
    let text = text.strip_prefix("```").unwrap_or(text);
    let text = text.trim_end_matches("```").trim();
    let re_obj = regex::Regex::new(r",\s*}").unwrap();
    let re_arr = regex::Regex::new(r",\s*]").unwrap();
    let fixed = re_obj.replace_all(text, "}");
    re_arr.replace_all(&fixed, "]").to_string()
}

/// 将 JSON 数组转为 f64 向量
fn parse_vec(value: &Value) -> anyhow::Result<Vec<f64>> {
    let arr = value
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("embedding 元素不是数组"))?;
    Ok(arr.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect())
}

/// 从错误响应中提取可读信息：
/// Ollama 为 `{"error": "..."}`，OpenAI 为 `{"error": {"message": "..."}}`
fn describe_error(text: &str) -> String {
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        if let Some(s) = v["error"].as_str() {
            return s.to_string();
        }
        if let Some(s) = v["error"]["message"].as_str() {
            return s.to_string();
        }
        if let Some(s) = v["message"].as_str() {
            return s.to_string();
        }
    }
    text.chars().take(200).collect()
}

fn empty_metadata() -> Value {
    json!({
        "language": "",
        "type": "",
        "title": "",
        "title_en": "",
        "year": null,
        "authors": [],
        "authors_en": [],
        "abstract": "",
        "abstract_en": "",
        "doi": "",
        "source": "",
        "journal": "",
        "journal_en": "",
        "keywords": [],
        "keywords_en": [],
        "category": ""
    })
}

fn parse_json_from_text(text: &str) -> Value {
    let text = text.trim();
    let text = if text.starts_with("```json") {
        &text[7..]
    } else if text.starts_with("```") {
        &text[3..]
    } else {
        text
    };
    let text = text.trim_end_matches("```").trim();

    // Try direct parse
    if let Ok(v) = serde_json::from_str::<Value>(text) {
        return v;
    }

    // Try to find JSON block
    if let Some(brace_start) = text.find('{') {
        let mut depth = 0;
        for (i, ch) in text[brace_start..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        let json_str = &text[brace_start..=brace_start + i];
                        // Try to fix trailing commas
                        let fixed = regex::Regex::new(r",\s*}").unwrap()
                            .replace_all(json_str, "}");
                        let fixed = regex::Regex::new(r",\s*]").unwrap()
                            .replace_all(&fixed, "]");
                        if let Ok(v) = serde_json::from_str::<Value>(&fixed) {
                            return v;
                        }
                        break;
                    }
                }
                _ => {}
            }
        }
    }

    empty_metadata()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_prompt_uses_default_template_and_placeholders() {
        let existing = json!({"title": "旧标题", "year": 1999});
        let prompt = build_extract_prompt("", Some(&existing), "正文内容", 100);
        // 默认模板：占位符被替换，且不再残留
        assert!(!prompt.contains("{content}"));
        assert!(!prompt.contains("{metadata}"));
        assert!(prompt.contains("正文内容"));
        assert!(prompt.contains("旧标题"));
        // 纠错规则（正文找不到依据时留空）写在提示词里
        assert!(prompt.contains("留空"));
    }

    #[test]
    fn extract_prompt_custom_template_and_truncation() {
        let prompt = build_extract_prompt(
            "元数据={metadata}",
            None,
            &"字".repeat(50),
            10,
        );
        // 自定义模板缺少 {content} 时，自动在末尾补正文
        assert!(prompt.starts_with("元数据=（空）"));
        assert!(prompt.contains(&"字".repeat(10)));
        assert!(!prompt.contains(&"字".repeat(11)));
    }

    #[test]
    fn extract_prompt_keeps_body_when_template_has_content_placeholder() {
        let prompt = build_extract_prompt("前\n{content}\n后", None, "BODY", 100);
        assert_eq!(prompt, "前\nBODY\n后");
    }

    #[test]
    fn protocol_parse_falls_back_to_ollama() {
        assert_eq!(Protocol::parse("openai"), Protocol::OpenAi);
        assert_eq!(Protocol::parse("OpenAI"), Protocol::OpenAi);
        assert_eq!(Protocol::parse(""), Protocol::Ollama);
        assert_eq!(Protocol::parse("anything"), Protocol::Ollama);
    }

    #[test]
    fn openai_base_url_gets_v1_suffix() {
        let c = LlmClient::new(Protocol::OpenAi, "https://api.openai.com", None, "gpt-4o-mini");
        assert_eq!(c.base(), "https://api.openai.com/v1");
        let c = LlmClient::new(Protocol::OpenAi, "http://127.0.0.1:11434/v1/", None, "gpt");
        assert_eq!(c.base(), "http://127.0.0.1:11434/v1");
        let c = LlmClient::new(Protocol::Ollama, "http://127.0.0.1:11434/", None, "qwen");
        assert_eq!(c.base(), "http://127.0.0.1:11434");
    }

    #[test]
    fn describe_error_handles_both_protocols() {
        assert_eq!(describe_error(r#"{"error":"model not found"}"#), "model not found");
        assert_eq!(
            describe_error(r#"{"error":{"message":"invalid api key","type":"auth"}}"#),
            "invalid api key"
        );
    }

    #[test]
    fn parses_generate_text_for_both_protocols() {
        let ollama = LlmClient::new(Protocol::Ollama, "http://h", None, "m");
        assert_eq!(
            ollama.parse_generate_text(&json!({"response": "hi", "done": true})),
            "hi"
        );
        let openai = LlmClient::new(Protocol::OpenAi, "http://h", None, "m");
        assert_eq!(
            openai.parse_generate_text(&json!({"choices": [{"message": {"content": "hi"}}]})),
            "hi"
        );
    }

    #[test]
    fn parses_ollama_embeddings_shape() {
        let c = LlmClient::new(Protocol::Ollama, "http://h", None, "m");
        // Ollama 的 embeddings 元素是裸数组，不是 {"embedding": [...]}
        let data = json!({"embeddings": [[1.0, 2.0], [3.0, 4.0]]});
        assert_eq!(
            c.parse_embeddings(&data).unwrap(),
            vec![vec![1.0, 2.0], vec![3.0, 4.0]]
        );
        assert!(c
            .parse_embeddings(&json!({"embeddings": [{"embedding": [1.0]}]}))
            .is_err());
    }

    #[test]
    fn parses_openai_embeddings_sorted_by_index() {
        let c = LlmClient::new(Protocol::OpenAi, "http://h", None, "m");
        let data = json!({"data": [
            {"index": 1, "embedding": [3.0, 4.0]},
            {"index": 0, "embedding": [1.0, 2.0]}
        ]});
        assert_eq!(
            c.parse_embeddings(&data).unwrap(),
            vec![vec![1.0, 2.0], vec![3.0, 4.0]]
        );
        assert!(c.parse_embeddings(&json!({"data": [{"index": 0}]})).is_err());
    }

    #[test]
    fn parses_chat_text_both_protocols() {
        let ollama = LlmClient::new(Protocol::Ollama, "http://h", None, "m");
        assert_eq!(
            ollama.parse_chat_text(&json!({"message": {"role": "assistant", "content": "hi"}})),
            "hi"
        );
        // Ollama /api/chat 与 /api/generate 的响应结构不同，不能混用
        assert_eq!(ollama.parse_chat_text(&json!({"response": "hi"})), "");

        let openai = LlmClient::new(Protocol::OpenAi, "http://h", None, "m");
        assert_eq!(
            openai.parse_chat_text(&json!({"choices": [{"message": {"content": "hi"}}]})),
            "hi"
        );
    }

    #[test]
    fn chat_message_role_validation() {
        assert!(ChatMessage::new("system", "s").is_valid_role());
        assert!(ChatMessage::new("user", "u").is_valid_role());
        assert!(ChatMessage::new("assistant", "a").is_valid_role());
        assert!(ChatMessage::new("tool", "t").is_valid_role());
        assert!(!ChatMessage::new("", "x").is_valid_role());
        assert!(!ChatMessage::new("function", "x").is_valid_role());
    }

    #[test]
    fn messages_to_value_adapts_tool_calls_per_protocol() {
        let call = ToolCall {
            id: "call_1".to_string(),
            name: "get_stats".to_string(),
            arguments: json!({"foo": 1}),
        };
        let msgs = vec![
            ChatMessage::assistant_tool_calls("", vec![call.clone()]),
            ChatMessage::tool_result(&call, "总文献数 3"),
        ];

        let ollama = LlmClient::new(Protocol::Ollama, "http://h", None, "m");
        let v = ollama.messages_to_value(&msgs);
        // Ollama：arguments 保持对象，tool 结果消息带 tool_name
        assert_eq!(v[0]["tool_calls"][0]["function"]["arguments"], json!({"foo": 1}));
        assert_eq!(v[1]["role"], "tool");
        assert_eq!(v[1]["tool_name"], "get_stats");
        assert!(v[1].get("tool_call_id").is_none());

        let openai = LlmClient::new(Protocol::OpenAi, "http://h", None, "m");
        let v = openai.messages_to_value(&msgs);
        // OpenAI：arguments 是 JSON 字符串，tool 结果消息带 tool_call_id
        assert_eq!(v[0]["tool_calls"][0]["id"], "call_1");
        assert_eq!(v[0]["tool_calls"][0]["type"], "function");
        assert_eq!(v[0]["tool_calls"][0]["function"]["arguments"], "{\"foo\":1}");
        assert_eq!(v[1]["tool_call_id"], "call_1");
    }

    #[test]
    fn parses_tool_calls_from_both_protocols() {
        let ollama = LlmClient::new(Protocol::Ollama, "http://h", None, "m");
        // Ollama 不返回 id、arguments 为对象
        let calls = ollama.parse_tool_calls(&json!({
            "message": {"content": "", "tool_calls": [
                {"function": {"name": "search_info", "arguments": {"query": "钠离子"}}}
            ]}
        }));
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "search_info");
        assert_eq!(calls[0].arguments["query"], "钠离子");

        let openai = LlmClient::new(Protocol::OpenAi, "http://h", None, "m");
        // OpenAI 返回 id、arguments 为 JSON 字符串
        let calls = openai.parse_tool_calls(&json!({
            "choices": [{"message": {"content": null, "tool_calls": [
                {"id": "call_abc", "type": "function", "function": {"name": "read_markdown", "arguments": "{\"id\":7,\"section\":\"Introduction\"}"}}
            ]}}]
        }));
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_abc");
        assert_eq!(calls[0].name, "read_markdown");
        assert_eq!(calls[0].arguments["id"], 7);

        // 无 tool_calls / 参数非法时不报错
        assert!(ollama.parse_tool_calls(&json!({"message": {"content": "hi"}})).is_empty());
        assert_eq!(normalize_arguments(&json!("not-json")), json!({}));
    }

    #[test]
    fn tool_specs_map_input_schema_to_parameters() {
        let c = LlmClient::new(Protocol::OpenAi, "http://h", None, "m");
        let specs = c.tool_specs(&[json!({
            "name": "get_stats",
            "description": "统计",
            "inputSchema": {"type": "object", "properties": {}}
        })]);
        assert_eq!(specs[0]["type"], "function");
        assert_eq!(specs[0]["function"]["name"], "get_stats");
        assert_eq!(specs[0]["function"]["parameters"]["type"], "object");
    }

    #[test]
    fn text_tool_prompt_lists_tools_and_format() {
        let prompt = text_tool_prompt(&[json!({
            "name": "get_stats",
            "description": "统计",
            "inputSchema": {"type": "object", "properties": {}}
        })]);
        assert!(prompt.contains("get_stats"));
        assert!(prompt.contains("tool_calls"));
        assert!(prompt.contains("parameters"));
    }

    #[test]
    fn parses_text_tool_calls_in_various_shapes() {
        // 推荐格式：裸 JSON
        let calls = parse_text_tool_calls(r#"{"tool_calls":[{"name":"get_stats","arguments":{}}]}"#);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "get_stats");

        // 代码块 + 前后噪声 + 多调用
        let calls = parse_text_tool_calls(
            "好的，我先查一下：\n```json\n{\"tool_calls\":[{\"name\":\"get_stats\",\"arguments\":{}},{\"name\":\"read_markdown\",\"arguments\":{\"id\":3,\"line_count\":20}}]}\n```",
        );
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].name, "read_markdown");
        assert_eq!(calls[1].arguments["id"], 3);

        // 单调用写法与 arguments 为字符串的写法
        let calls = parse_text_tool_calls(r#"{"tool":"get_stats","arguments":"{}"}"#);
        assert_eq!(calls[0].name, "get_stats");
        let calls = parse_text_tool_calls(r#"{"name":"get_stats","arguments":{}}"#);
        assert_eq!(calls[0].name, "get_stats");
        // 尾随逗号容错
        let calls = parse_text_tool_calls(r#"{"tool_calls":[{"name":"get_stats","arguments":{},},],}"#);
        assert_eq!(calls[0].name, "get_stats");

        // 普通自然语言回答不应被误判为工具调用
        assert!(parse_text_tool_calls("库里共有 3 篇文献 [id=12]。").is_empty());
        assert!(parse_text_tool_calls("{\"total\": 3}").is_empty());
    }

    #[test]
    fn json_object_candidates_skips_braces_inside_strings() {
        let text = r#"{"a": "} 不是结束", "b": {"c": 1}}"#;
        let found = json_object_candidates(text);
        assert_eq!(found.len(), 1);
        assert!(serde_json::from_str::<Value>(&found[0]).is_ok());
    }
}
