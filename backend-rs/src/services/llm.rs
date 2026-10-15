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

const EXTRACT_PROMPT: &str = r#"Extract metadata from the following academic document content. Return strictly in JSON format without any additional text.

Important Rules:
1. First determine the primary language of the document (language field) using ISO 639-1 codes:
   - en = English
   - zh = Chinese
   - ja = Japanese
   - fr = French
   - ru = Russian
   - de = German
   - ko = Korean
   - es = Spanish
   - pt = Portuguese
   - ar = Arabic
   - Use the corresponding ISO 639-1 code for other languages

2. For non-English documents (language != "en"), provide both original and English versions:
   - title: Original title
   - title_en: English title
   - authors: Original author names (list)
   - authors_en: English author names (romanized or translated)
   - abstract: Original abstract
   - abstract_en: English abstract
   - journal: Original journal/conference name
   - journal_en: English journal/conference name
   - keywords: Original keywords (list)
   - keywords_en: English keywords (list)

3. For English documents (language="en"), leave *_en fields empty

Return the following JSON format:
{{"language": "language_code", "type": "journalArticle|book|conferencePaper|thesis|report|preprint|bookSection|webpage|document|patent|review|manuscript|presentation", "title": "Title (original)", "title_en": "English title (required for non-English)", "year": publication_year (integer), "authors": ["Author1", "Author2"], "authors_en": ["Author1", "Author2"], "abstract": "Abstract (original)", "abstract_en": "English abstract (required for non-English)", "doi": "DOI", "source": "Source", "journal": "Journal/Conference name (original)", "journal_en": "English journal/conference (required for non-English)", "keywords": ["keyword1", "keyword2"], "keywords_en": ["keyword1", "keyword2"], "category": "Category"}}

Note: type must be one of the Zotero item types listed above. Use "journalArticle" for journal papers (not "article"), "conferencePaper" for conference papers (not "conference").

Document Content:
{content}"#;

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
            Protocol::OpenAi => data["choices"]
                .as_array()
                .and_then(|choices| choices.first())
                .and_then(|c| c["message"]["content"].as_str())
                .unwrap_or("")
                .to_string(),
        }
    }

    /// 从 Markdown 内容提取元数据
    pub async fn extract_metadata(&self, markdown_content: &str) -> anyhow::Result<Value> {
        let truncated: String = markdown_content.chars().take(4000).collect();
        let prompt = EXTRACT_PROMPT.replace("{content}", &truncated);

        let response_text = self.generate(&prompt, 0.1, 1024).await?;
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
}
