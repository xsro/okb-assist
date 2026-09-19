//! MinerU PDF 解析服务。
//!
//! 支持两种模式：
//! - 本地模式（type=local）：连接本地 MinerU 服务器（如 http://127.0.0.1:8002）
//! - 官方精准解析 API（type=official）：调用 MinerU 官方 API（https://mineru.net）

use std::io::{Cursor, Read, Write};
use std::path::Path;

use serde_json::{json, Value};

use crate::config::MinerUConfig;

/// MinerU 模式类型
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MineruType {
    /// 本地 MinerU 服务器
    Local,
    /// 官方精准解析 API
    Official,
}

impl MineruType {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "official" | "精准解析" | "精准" => MineruType::Official,
            _ => MineruType::Local,
        }
    }
}

pub struct MinerUClient {
    base_url: String,
    key: String,
    mineru_type: MineruType,
    model_version: String,
    http: reqwest::Client,
}

impl MinerUClient {
    pub fn new(config: &MinerUConfig) -> Self {
        Self {
            base_url: config.url.clone(),
            key: config.key.clone(),
            mineru_type: MineruType::from_str(&config.mineru_type),
            model_version: config.model_version.clone(),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .connect_timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    pub fn from_config(config: &MinerUConfig) -> Self {
        Self::new(config)
    }

    // ── 提交解析任务 ──────────────────────────────────────

    pub async fn submit_parse_task(&self, file_path: &str) -> anyhow::Result<String> {
        match self.mineru_type {
            MineruType::Local => self.submit_parse_task_local(file_path).await,
            MineruType::Official => self.submit_parse_task_official(file_path).await,
        }
    }

    /// 本地模式：提交 PDF 解析任务（multipart 上传文件）
    async fn submit_parse_task_local(&self, file_path: &str) -> anyhow::Result<String> {
        let pdf_bytes = tokio::fs::read(file_path).await?;
        let file_name = Path::new(file_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "document.pdf".to_string());

        let part = reqwest::multipart::Part::bytes(pdf_bytes)
            .file_name(file_name)
            .mime_str("application/pdf")?;

        let form = reqwest::multipart::Form::new()
            .part("files", part)
            .text("return_md", "true")
            .text("backend", "pipeline")
            .text("parse_method", "auto")
            .text("formula_enable", "true")
            .text("table_enable", "true")
            .text("image_analysis", "false")
            .text("response_format_zip", "true")
            .text("return_images", "true");

        let resp = self.http.post(format!("{}/tasks", self.base_url))
            .bearer_auth(&self.key)
            .multipart(form)
            .send().await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("MinerU submit failed: HTTP {} - {}", status, text.chars().take(200).collect::<String>());
        }

        let data: Value = resp.json().await?;
        let task_id = data["task_id"].as_str()
            .ok_or_else(|| anyhow::anyhow!("No task_id in response"))?
            .to_string();
        Ok(task_id)
    }

    /// 官方精准解析 API：通过批量上传接口提交单个文件
    async fn submit_parse_task_official(&self, file_path: &str) -> anyhow::Result<String> {
        let pdf_bytes = tokio::fs::read(file_path).await?;
        let file_name = Path::new(file_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "document.pdf".to_string());

        // Step 1: 申请上传 URL
        let body = json!({
            "files": [{"name": file_name}],
            "model_version": self.model_version,
            "enable_formula": true,
            "enable_table": true,
        });

        let resp = self.http.post(format!("{}/api/v4/file-urls/batch", self.base_url))
            .bearer_auth(&self.key)
            .json(&body)
            .send().await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("MinerU official submit failed: HTTP {} - {}", status, text.chars().take(200).collect::<String>());
        }

        let data: Value = resp.json().await?;
        if data["code"].as_i64() != Some(0) {
            let msg = data["msg"].as_str().unwrap_or("unknown error");
            anyhow::bail!("MinerU official submit failed: {}", msg);
        }

        let batch_id = data["data"]["batch_id"].as_str()
            .ok_or_else(|| anyhow::anyhow!("No batch_id in response"))?
            .to_string();

        let upload_url = data["data"]["file_urls"][0].as_str()
            .ok_or_else(|| anyhow::anyhow!("No upload URL in response"))?;

        // Step 2: 上传文件到 OSS
        let upload_resp = self.http.put(upload_url)
            .body(pdf_bytes)
            .send().await?;

        if !upload_resp.status().is_success() {
            let status = upload_resp.status();
            anyhow::bail!("MinerU official file upload failed: HTTP {}", status);
        }

        Ok(batch_id)
    }

    // ── 检查任务状态 ──────────────────────────────────────

    pub async fn check_task_status(&self, task_id: &str) -> Value {
        match self.mineru_type {
            MineruType::Local => self.check_task_status_local(task_id).await,
            MineruType::Official => self.check_task_status_official(task_id).await,
        }
    }

    async fn check_task_status_local(&self, task_id: &str) -> Value {
        let resp = self.http.get(format!("{}/tasks/{}", self.base_url, task_id))
            .bearer_auth(&self.key)
            .send().await;

        match resp {
            Ok(resp) if resp.status().is_success() => {
                resp.json().await.unwrap_or(json!({"status": "unknown"}))
            }
            _ => json!({"status": "unknown"}),
        }
    }

    async fn check_task_status_official(&self, task_id: &str) -> Value {
        let resp = self.http
            .get(format!("{}/api/v4/extract-results/batch/{}", self.base_url, task_id))
            .bearer_auth(&self.key)
            .send()
            .await;

        match resp {
            Ok(resp) if resp.status().is_success() => {
                let data: Value = resp.json().await.unwrap_or(json!({}));
                let results = data["data"]["extract_result"].as_array();
                if let Some(results) = results {
                    if let Some(first) = results.first() {
                        let state = first["state"].as_str().unwrap_or("unknown");
                        let normalized = match state {
                            "done" => "completed",
                            "failed" => "failed",
                            "pending" => "pending",
                            "running" => "processing",
                            "converting" => "processing",
                            "waiting-file" => "pending",
                            _ => state,
                        };
                        return json!({
                            "status": normalized,
                            "state": state,
                            "full_zip_url": first["full_zip_url"].clone(),
                            "err_msg": first["err_msg"].clone(),
                            "extract_progress": first["extract_progress"].clone(),
                        });
                    }
                }
                json!({"status": "unknown"})
            }
            _ => json!({"status": "unknown"}),
        }
    }

    // ── 轮询任务直到完成 ──────────────────────────────────

    pub async fn poll_task(&self, task_id: &str, timeout_secs: u64) -> anyhow::Result<Value> {
        let poll_interval = 2u64;
        let max_polls = timeout_secs / poll_interval;

        for _ in 0..max_polls {
            let status_result = self.check_task_status(task_id).await;
            let status = status_result["status"].as_str().unwrap_or("unknown");

            match status {
                "completed" => return Ok(status_result),
                "failed" => {
                    let error = status_result["err_msg"].as_str().unwrap_or("Unknown error");
                    anyhow::bail!("MinerU task failed: {}", error);
                }
                _ => {
                    tokio::time::sleep(std::time::Duration::from_secs(poll_interval)).await;
                }
            }
        }

        anyhow::bail!("MinerU task timed out: {}", task_id);
    }

    // ── 获取任务结果 ──────────────────────────────────────

    pub async fn get_task_result(
        &self,
        task_id: &str,
        output_dir: &str,
        doc_id: Option<i64>,
    ) -> anyhow::Result<String> {
        match self.mineru_type {
            MineruType::Local => self.get_task_result_local(task_id, output_dir, doc_id).await,
            MineruType::Official => self.get_task_result_official(task_id, output_dir, doc_id).await,
        }
    }

    async fn get_task_result_local(
        &self,
        task_id: &str,
        output_dir: &str,
        doc_id: Option<i64>,
    ) -> anyhow::Result<String> {
        let resp = self.http.get(format!("{}/tasks/{}/result", self.base_url, task_id))
            .bearer_auth(&self.key)
            .send().await?;

        if resp.status() != 200 {
            anyhow::bail!("MinerU result fetch failed: HTTP {}", resp.status());
        }

        let zip_bytes = resp.bytes().await?;
        let output_dir = output_dir.to_string();
        tokio::task::spawn_blocking(move || {
            process_zip_result(&zip_bytes, &output_dir, doc_id)
        })
        .await?
    }

    async fn get_task_result_official(
        &self,
        task_id: &str,
        output_dir: &str,
        doc_id: Option<i64>,
    ) -> anyhow::Result<String> {
        // 查询批量结果获取 full_zip_url
        let resp = self.http
            .get(format!("{}/api/v4/extract-results/batch/{}", self.base_url, task_id))
            .bearer_auth(&self.key)
            .send()
            .await?;

        if !resp.status().is_success() {
            anyhow::bail!("MinerU official result fetch failed: HTTP {}", resp.status());
        }

        let data: Value = resp.json().await?;
        let results = data["data"]["extract_result"].as_array()
            .ok_or_else(|| anyhow::anyhow!("No results in batch response"))?;

        let first = results.first()
            .ok_or_else(|| anyhow::anyhow!("Empty results array"))?;

        let state = first["state"].as_str().unwrap_or("");
        if state != "done" {
            let err = first["err_msg"].as_str().unwrap_or("Unknown error");
            anyhow::bail!("MinerU task not completed: {} (state: {})", err, state);
        }

        let zip_url = first["full_zip_url"].as_str()
            .ok_or_else(|| anyhow::anyhow!("No full_zip_url in result"))?;

        // 下载 zip 压缩包
        let zip_resp = self.http.get(zip_url).send().await?;
        if !zip_resp.status().is_success() {
            anyhow::bail!("MinerU official zip download failed: HTTP {}", zip_resp.status());
        }

        let zip_bytes = zip_resp.bytes().await?;
        let output_dir = output_dir.to_string();
        tokio::task::spawn_blocking(move || {
            process_zip_result(&zip_bytes, &output_dir, doc_id)
        })
        .await?
    }
}

/// 处理 MinerU 返回的 zip 结果（同步函数，在 spawn_blocking 中运行）
fn process_zip_result(
    zip_bytes: &[u8],
    output_dir: &str,
    doc_id: Option<i64>,
) -> anyhow::Result<String> {
    std::fs::create_dir_all(output_dir)?;

    let cursor = Cursor::new(zip_bytes);
    let mut archive = zip::ZipArchive::new(cursor)?;

    let mut markdown_content = String::new();
    let mut image_entries: Vec<(String, Vec<u8>)> = Vec::new();

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let name = file.name().to_string();

        if name.ends_with(".md") {
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)?;
            markdown_content = String::from_utf8_lossy(&buf).to_string();
        } else if name.contains('/') {
            let filename = Path::new(&name)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if !filename.is_empty() && filename.len() > 3 {
                let lower = filename.to_lowercase();
                if lower.ends_with(".png") || lower.ends_with(".jpg")
                    || lower.ends_with(".jpeg") || lower.ends_with(".gif")
                    || lower.ends_with(".svg") {
                    let mut buf = Vec::new();
                    file.read_to_end(&mut buf)?;
                    image_entries.push((filename, buf));
                }
            }
        }
    }

    if markdown_content.is_empty() {
        anyhow::bail!("No markdown content found in MinerU result");
    }

    // Save images to images.zip
    if !image_entries.is_empty() {
        let images_zip_path = Path::new(output_dir).join("images.zip");
        let zip_file = std::fs::File::create(&images_zip_path)?;
        let mut zip_writer = zip::ZipWriter::new(zip_file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (filename, data) in &image_entries {
            zip_writer.start_file(filename, options)?;
            zip_writer.write_all(data)?;
        }
        zip_writer.finish()?;
    }

    // Save markdown
    let md_filename = match doc_id {
        Some(id) => format!("{}.md", id),
        None => "output.md".to_string(),
    };
    let md_path = Path::new(output_dir).join(&md_filename);
    std::fs::write(&md_path, &markdown_content)?;

    Ok(md_path.to_string_lossy().to_string())
}

/// 便捷函数：解析 PDF（提交 → 轮询 → 获取结果）
pub async fn parse_pdf(
    client: &MinerUClient,
    file_path: &str,
    output_dir: &str,
    doc_id: Option<i64>,
    timeout_secs: u64,
) -> anyhow::Result<String> {
    let task_id = client.submit_parse_task(file_path).await?;
    client.poll_task(&task_id, timeout_secs).await?;
    client.get_task_result(&task_id, output_dir, doc_id).await
}

/// 尝试多个 MinerU 配置，逐个解析直到成功
/// 返回 (使用的配置索引, 结果路径)
pub async fn parse_pdf_with_fallback(
    configs: &[MinerUConfig],
    file_path: &str,
    output_dir: &str,
    doc_id: Option<i64>,
) -> anyhow::Result<(usize, String)> {
    let mut last_error = String::new();
    for (i, config) in configs.iter().enumerate() {
        let client = MinerUClient::new(config);
        let timeout = config.task_timeout;
        match parse_pdf(&client, file_path, output_dir, doc_id, timeout).await {
            Ok(path) => return Ok((i, path)),
            Err(e) => {
                last_error = e.to_string();
                tracing::warn!(
                    "MinerU 配置 #{} ({}) 解析失败: {}，尝试下一个",
                    i, config.url, last_error
                );
            }
        }
    }
    anyhow::bail!("所有 MinerU 配置均解析失败: {}", last_error)
}