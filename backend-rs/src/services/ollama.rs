//! Ollama LLM / Embedding 服务。

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

pub struct OllamaClient {
    base_url: String,
    key: Option<String>,
    model: String,
    http: reqwest::Client,
}

impl OllamaClient {
    pub fn new(base_url: &str, key: Option<&str>, model: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            key: key.map(String::from),
            model: model.to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// 从 Markdown 内容提取元数据
    pub async fn extract_metadata(&self, markdown_content: &str) -> anyhow::Result<Value> {
        let truncated: String = markdown_content.chars().take(4000).collect();
        let prompt = EXTRACT_PROMPT.replace("{content}", &truncated);

        let mut req = self.http.post(format!("{}/api/generate", self.base_url))
            .json(&json!({
                "model": self.model,
                "prompt": prompt,
                "stream": false,
                "options": {
                    "temperature": 0.1,
                    "num_predict": 1024,
                }
            }));

        if let Some(ref k) = self.key {
            req = req.header("Authorization", format!("Bearer {}", k));
        }

        let resp = req.send().await?;
        let status = resp.status();
        let text = resp.text().await?;

        if !status.is_success() {
            anyhow::bail!("Ollama returned error status {}: {}", status, text.chars().take(200).collect::<String>());
        }

        let result: Value = serde_json::from_str(&text)?;
        let response_text = result["response"].as_str().unwrap_or("").to_string();

        if response_text.is_empty() {
            return Ok(empty_metadata());
        }

        Ok(parse_json_from_text(&response_text))
    }

    /// 获取 embedding
    pub async fn get_embedding(&self, text: &str) -> anyhow::Result<Vec<f64>> {
        let resp = self.http.post(format!("{}/api/embed", self.base_url))
            .json(&json!({
                "model": self.model,
                "input": text,
            }))
            .send().await?;

        if !resp.status().is_success() {
            anyhow::bail!("Ollama embed error: HTTP {}", resp.status());
        }

        let data: Value = resp.json().await?;
        let embeddings = data["embeddings"].as_array()
            .ok_or_else(|| anyhow::anyhow!("No embeddings in response"))?;
        let first = embeddings.first()
            .ok_or_else(|| anyhow::anyhow!("Empty embeddings array"))?;
        
        let vec: Vec<f64> = first.as_array()
            .ok_or_else(|| anyhow::anyhow!("Embedding is not an array"))?
            .iter()
            .map(|v| v.as_f64().unwrap_or(0.0))
            .collect();
        
        Ok(vec)
    }

    /// 批量获取 embedding
    pub async fn get_embeddings_batch(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f64>>> {
        let resp = self.http.post(format!("{}/api/embed", self.base_url))
            .json(&json!({
                "model": self.model,
                "input": texts,
            }))
            .send().await?;

        if !resp.status().is_success() {
            anyhow::bail!("Ollama embed error: HTTP {}", resp.status());
        }

        let data: Value = resp.json().await?;
        let embeddings = data["embeddings"].as_array()
            .ok_or_else(|| anyhow::anyhow!("No embeddings in response"))?;
        
        let result: Vec<Vec<f64>> = embeddings.iter()
            .map(|e| e.as_array()
                .map(|arr| arr.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect())
                .unwrap_or_default())
            .collect();
        
        Ok(result)
    }

    /// 列出可用模型
    pub async fn list_models(&self) -> anyhow::Result<Vec<String>> {
        let resp = self.http.get(format!("{}/api/tags", self.base_url))
            .send().await?;

        if !resp.status().is_success() {
            anyhow::bail!("Ollama tags error: HTTP {}", resp.status());
        }

        let data: Value = resp.json().await?;
        let models = data["models"].as_array()
            .map(|arr| arr.iter()
                .filter_map(|m| m["name"].as_str().map(String::from))
                .collect())
            .unwrap_or_default();
        Ok(models)
    }
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