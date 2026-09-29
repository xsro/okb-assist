//! MinerU PDF 解析服务
//!
//! 支持三种 API 接口，由 config.json 的 type 字段选择：
//!
//! 1. **local**（自部署 V1 API）：连接本地 MinerU 服务（mineru-kit api-server）
//!    端点：http://host:port/v1/...
//!    流程：创建上传 → 上传 → 完成 → 提交任务 → 轮询 → 下载
//!
//! 2. **official**（官方云 V4 精准解析 API）：通过 Token 认证，高精度解析
//!    端点：https://mineru.net/api/v4/...
//!    流程：获取上传 URL → 上传文件 → 系统自动提交 → 轮询任务 → 下载 ZIP
//!
//! 3. **official-lightweight**（官方云 V1 Agent 轻量解析 API）：无需 Token，IP 限频
//!    端点：https://mineru.net/api/v1/agent/parse/...
//!    流程：上传文件 → 轮询任务 → 下载 Markdown

use std::collections::HashMap;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use serde_json::{json, Value};

use crate::config::MinerUConfig;

/// MinerU 模式类型
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MineruType {
    /// 自部署 V1 API 服务（type=local）
    Local,
    /// 官方云 V4 精准解析 API（type=official）
    Official,
    /// 官方云 V1 Agent 轻量解析 API（type=official-lightweight）
    OfficialLightweight,
}

impl MineruType {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().replace("-", "_").as_str() {
            "official" | "精准解析" | "精准" => MineruType::Official,
            "official_lightweight" | "official-lightweight" | "轻量解析" | "轻量" | "agent" => {
                MineruType::OfficialLightweight
            }
            _ => MineruType::Local,
        }
    }
}

/// 任务状态标准化结果
struct TaskStatus {
    done: bool,
    error: Option<String>,
    /// V1 API: output_files 列表；V4 API: full_zip_url；轻量 API: markdown_url
    result_data: Value,
}

pub struct MinerUClient {
    base_url: String,
    key: String,
    mineru_type: MineruType,
    tier: String,
    http: reqwest::Client,
}

impl MinerUClient {
    pub fn new(config: &MinerUConfig) -> Self {
        let mineru_type = MineruType::from_str(&config.mineru_type);
        let base_url = Self::build_base_url(&config.url, mineru_type);
        Self {
            base_url,
            key: config.key.clone(),
            mineru_type,
            tier: config.tier.clone(),
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .connect_timeout(std::time::Duration::from_secs(15))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    pub fn from_config(config: &MinerUConfig) -> Self {
        Self::new(config)
    }

    /// 根据类型构建正确的 API 基础 URL
    fn build_base_url(url: &str, mineru_type: MineruType) -> String {
        let url = url.trim_end_matches('/').to_string();
        match mineru_type {
            // 自部署：base_url 直接为 http://host:port
            MineruType::Local => url,
            // V4/Official：https://mineru.net → https://mineru.net/api
            MineruType::Official | MineruType::OfficialLightweight => {
                if !url.ends_with("/api") {
                    format!("{}/api", url)
                } else {
                    url
                }
            }
        }
    }

    // ════════════════════════════════════════════════════════
    //  公开接口（统一签名，路由到不同实现）
    // ════════════════════════════════════════════════════════

    /// 提交解析任务，返回 task_id / job_id
    pub async fn submit_parse_task(&self, file_path: &str) -> anyhow::Result<String> {
        match self.mineru_type {
            MineruType::Local => self.submit_parse_task_v1(file_path).await,
            MineruType::Official => self.submit_parse_task_v4(file_path).await,
            MineruType::OfficialLightweight => self.submit_parse_task_lightweight(file_path).await,
        }
    }

    /// 检查任务状态，返回标准化 Value
    pub async fn check_task_status(&self, task_id: &str) -> Value {
        match self.mineru_type {
            MineruType::Local => self.check_task_status_v1(task_id).await,
            MineruType::Official => self.check_task_status_v4(task_id).await,
            MineruType::OfficialLightweight => self.check_task_status_lightweight(task_id).await,
        }
    }

    /// 轮询任务直到完成
    pub async fn poll_task(&self, task_id: &str, timeout_secs: u64) -> anyhow::Result<Value> {
        let poll_interval = match self.mineru_type {
            MineruType::Local => 2u64,
            MineruType::Official => 5u64,
            MineruType::OfficialLightweight => 3u64,
        };
        let max_polls = timeout_secs / poll_interval;

        for _ in 0..max_polls {
            let status_result = self.check_task_status(task_id).await;
            if let Some(done) = status_result.get("_done").and_then(|v| v.as_bool()) {
                if done {
                    return Ok(status_result);
                }
            }

            let status = status_result["status"].as_str().unwrap_or("unknown");
            match status {
                "failed" => {
                    let error = status_result["error"]
                        .as_str()
                        .unwrap_or("Unknown error");
                    anyhow::bail!("MinerU task failed: {}", error);
                }
                "canceled" => {
                    anyhow::bail!("MinerU task was canceled");
                }
                _ => {
                    tokio::time::sleep(std::time::Duration::from_secs(poll_interval)).await;
                }
            }
        }

        anyhow::bail!("MinerU task timed out: {}", task_id);
    }

    /// 获取任务结果，返回 markdown 文件路径
    pub async fn get_task_result(
        &self,
        task_id: &str,
        output_dir: &str,
        doc_id: Option<i64>,
    ) -> anyhow::Result<String> {
        match self.mineru_type {
            MineruType::Local => self.get_task_result_v1(task_id, output_dir, doc_id).await,
            MineruType::Official => self.get_task_result_v4(task_id, output_dir, doc_id).await,
            MineruType::OfficialLightweight => {
                self.get_task_result_lightweight(task_id, output_dir, doc_id)
                    .await
            }
        }
    }

    // ════════════════════════════════════════════════════════
    //  实现：V1 自部署 API（type=local）
    // ════════════════════════════════════════════════════════

    async fn submit_parse_task_v1(&self, file_path: &str) -> anyhow::Result<String> {
        let file_name = Path::new(file_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "document.pdf".to_string());

        let pdf_bytes = tokio::fs::read(file_path).await?;
        let (upload_id, upload_method, upload_url, upload_headers) =
            self.v1_create_upload(&file_name, pdf_bytes.len() as u64, &pdf_bytes).await?;

        if upload_method != "SKIP" {
            self.v1_upload_file(&upload_method, &upload_url, &upload_headers, pdf_bytes)
                .await?;
        }

        let file_id = self.v1_complete_upload(&upload_id).await?;
        let job_id = self.v1_submit_job(&file_id).await?;
        Ok(job_id)
    }

    /// V1: 创建上传
    async fn v1_create_upload(
        &self,
        filename: &str,
        file_size: u64,
        file_bytes: &[u8],
    ) -> anyhow::Result<(String, String, String, HashMap<String, String>)> {
        use sha2::Digest;
        let file_bytes_vec = file_bytes.to_vec();
        let sha256_hex = tokio::task::spawn_blocking(move || {
            let sha256 = sha2::Sha256::digest(&file_bytes_vec);
            hex::encode(sha256)
        })
        .await
        .map_err(|e| anyhow::anyhow!("SHA-256 计算失败: {}", e))?;

        let body = json!({
            "filename": filename,
            "bytes": file_size,
            "sha256sum": sha256_hex,
            "mime_type": "application/pdf",
            "purpose": "parse",
        });

        let resp = self
            .http
            .post(format!("{}/v1/uploads", self.base_url))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("MinerU create upload failed: HTTP {} - {}", status, text.chars().take(200).collect::<String>());
        }

        let data: Value = resp.json().await?;

        if data.get("file").and_then(|v| v.as_object()).is_some() {
            let file_id = data["file"]["id"].as_str()
                .ok_or_else(|| anyhow::anyhow!("No file.id in dedup response"))?
                .to_string();
            return Ok((file_id, "SKIP".to_string(), String::new(), HashMap::new()));
        }

        let upload_id = data["id"].as_str()
            .ok_or_else(|| anyhow::anyhow!("No id in response"))?
            .to_string();
        let upload_method = data["upload_method"].as_str().unwrap_or("PUT").to_string();
        let upload_url = data["upload_url"].as_str()
            .ok_or_else(|| anyhow::anyhow!("No upload_url in response"))?
            .to_string();

        let mut upload_headers = HashMap::new();
        if let Some(headers) = data["upload_headers"].as_object() {
            for (k, v) in headers {
                if let Some(val) = v.as_str() {
                    upload_headers.insert(k.clone(), val.to_string());
                }
            }
        }

        Ok((upload_id, upload_method, upload_url, upload_headers))
    }

    /// V1: 上传文件字节
    async fn v1_upload_file(
        &self,
        method: &str,
        url: &str,
        headers: &HashMap<String, String>,
        data: Vec<u8>,
    ) -> anyhow::Result<()> {
        if method == "SKIP" {
            return Ok(());
        }

        let full_url = resolve_url(&self.base_url, url);
        let mut req = match method.to_uppercase().as_str() {
            "PUT" => self.http.put(&full_url),
            "POST" => self.http.post(&full_url),
            _ => self.http.put(&full_url),
        };

        for (k, v) in headers {
            req = req.header(k.as_str(), v.as_str());
        }

        if !self.key.is_empty() && is_same_origin(&full_url, &self.base_url) {
            req = req.bearer_auth(&self.key);
        }

        let resp = req.body(data).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("MinerU file upload failed: HTTP {}", resp.status());
        }
        Ok(())
    }

    /// V1: 完成上传 → file_id
    async fn v1_complete_upload(&self, upload_id: &str) -> anyhow::Result<String> {
        let resp = self
            .http
            .post(format!("{}/v1/uploads/{}/complete", self.base_url, upload_id))
            .bearer_auth(&self.key)
            .json(&json!({}))
            .send()
            .await?;

        if !resp.status().is_success() {
            anyhow::bail!("MinerU complete upload failed: HTTP {}", resp.status());
        }
        let data: Value = resp.json().await?;
        let file_id = data["file"]["id"].as_str()
            .ok_or_else(|| anyhow::anyhow!("No file.id in complete response"))?
            .to_string();
        Ok(file_id)
    }

    /// V1: 提交解析任务 → job_id
    async fn v1_submit_job(&self, file_id: &str) -> anyhow::Result<String> {
        let body = json!({
            "files": [{
                "source": { "type": "file_id", "file_id": file_id }
            }],
            "tier": self.tier,
            "output_formats": ["markdown", "zip"],
        });

        let resp = self
            .http
            .post(format!("{}/v1/parse/jobs", self.base_url))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("MinerU submit job failed: HTTP {} - {}", status, text.chars().take(200).collect::<String>());
        }
        let data: Value = resp.json().await?;
        Ok(data["job_id"].as_str()
            .ok_or_else(|| anyhow::anyhow!("No job_id in response"))?
            .to_string())
    }

    /// V1: 检查任务状态
    async fn check_task_status_v1(&self, job_id: &str) -> Value {
        let resp = self
            .http
            .get(format!("{}/v1/parse/jobs/{}", self.base_url, job_id))
            .bearer_auth(&self.key)
            .send()
            .await;

        match resp {
            Ok(resp) if resp.status().is_success() => {
                let data: Value = resp.json().await.unwrap_or(json!({}));
                let status = data["status"].as_str().unwrap_or("unknown");
                let normalized = match status {
                    "completed" => "completed",
                    "failed" => "failed",
                    "queued" => "pending",
                    "running" => "processing",
                    "processing" => "processing",
                    "partial" => "partial",
                    "canceled" => "canceled",
                    _ => status,
                };

                let files = data["files"].as_array().cloned().unwrap_or_default();
                let mut output_files: Vec<Value> = Vec::new();
                for file_entry in &files {
                    if let Some(of) = file_entry.get("output_files").and_then(|v| v.as_object()) {
                        for (fmt_name, fmt_val) in of {
                            if let Some(fid) = fmt_val.get("file_id").and_then(|v| v.as_str()) {
                                output_files.push(json!({
                                    "file_id": fid,
                                    "format": fmt_name,
                                    "bytes": fmt_val.get("bytes"),
                                }));
                            }
                        }
                    }
                }

                json!({
                    "_done": normalized == "completed" || normalized == "partial",
                    "status": normalized,
                    "output_files": output_files,
                    "error": data.get("last_error").or_else(|| data.get("error")).cloned().unwrap_or(json!(null)),
                })
            }
            _ => json!({"_done": false, "status": "unknown"}),
        }
    }

    /// V1: 获取任务结果
    async fn get_task_result_v1(
        &self,
        job_id: &str,
        output_dir: &str,
        doc_id: Option<i64>,
    ) -> anyhow::Result<String> {
        let status_result = self.check_task_status_v1(job_id).await;
        let status = status_result["status"].as_str().unwrap_or("");
        if status != "completed" && status != "partial" {
            anyhow::bail!("Task not completed (status: {})", status);
        }

        let output_files = status_result["output_files"].as_array().cloned().unwrap_or_default();
        if output_files.is_empty() {
            anyhow::bail!("No output files in job result");
        }

        std::fs::create_dir_all(output_dir)?;

        let mut zip_bytes: Option<Vec<u8>> = None;
        let mut standalone_markdown: Option<String> = None;

        for file_entry in &output_files {
            let format = file_entry["format"].as_str().unwrap_or("");
            let file_id = file_entry["file_id"].as_str().unwrap_or("");
            if file_id.is_empty() {
                continue;
            }

            let content = match self.v1_download_file(file_id).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("下载文件 {} 失败: {}", file_id, e);
                    continue;
                }
            };

            match format {
                "markdown" => standalone_markdown = Some(String::from_utf8_lossy(&content).to_string()),
                "zip" => zip_bytes = Some(content),
                _ => {}
            }
        }

        let (final_markdown, extracted_images) = if let Some(zip_data) = &zip_bytes {
            extract_from_zip(zip_data, output_dir)
        } else {
            (None, Vec::new())
        };

        let md = if let Some(ref md_from_zip) = final_markdown {
            md_from_zip.clone()
        } else if let Some(ref md_standalone) = standalone_markdown {
            md_standalone.clone()
        } else {
            anyhow::bail!("No markdown content found in MinerU result");
        };

        if !extracted_images.is_empty() {
            save_images_zip(output_dir, &extracted_images);
        }

        let md_filename = match doc_id {
            Some(id) => format!("{}.md", id),
            None => "output.md".to_string(),
        };
        let md_path = Path::new(output_dir).join(&md_filename);
        std::fs::write(&md_path, &md)?;
        Ok(md_path.to_string_lossy().to_string())
    }

    /// V1: 下载文件（支持 302 跳转）
    async fn v1_download_file(&self, file_id: &str) -> anyhow::Result<Vec<u8>> {
        let resp = self
            .http
            .get(format!("{}/v1/files/{}/content", self.base_url, file_id))
            .bearer_auth(&self.key)
            .send()
            .await?;

        if resp.status().is_success() {
            return Ok(resp.bytes().await?.to_vec());
        }

        if matches!(resp.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
            if let Some(location) = resp.headers().get("location").and_then(|v| v.to_str().ok()) {
                let redirect_resp = self.http.get(location).send().await?;
                if redirect_resp.status().is_success() {
                    return Ok(redirect_resp.bytes().await?.to_vec());
                }
                anyhow::bail!("MinerU file download redirect failed: HTTP {}", redirect_resp.status());
            }
        }

        anyhow::bail!("MinerU file download failed: HTTP {}", resp.status());
    }

    // ════════════════════════════════════════════════════════
    //  实现：V4 官方精准解析 API（type=official）
    // ════════════════════════════════════════════════════════

    /// V4: 通过批量文件上传提交任务
    /// 流程：获取预签名上传 URL → 上传文件 → 提交解析任务 → 轮询 → 获取结果
    /// 返回 submit_batch_id（用于后续轮询任务状态）
    async fn submit_parse_task_v4(&self, file_path: &str) -> anyhow::Result<String> {
        let file_name = Path::new(file_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "document.pdf".to_string());

        let data_id = format!("okb_{}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs());

        let pdf_bytes = tokio::fs::read(file_path).await?;

        // Step 1: 获取预签名上传 URL
        // 注意：model_version 参数已被官方 V4 API 移除，不再发送
        let body = json!({
            "files": [{
                "name": file_name,
                "data_id": data_id,
            }],
        });

        tracing::debug!("V4 请求上传 URL: {}", body.to_string());

        let resp = self
            .http
            .post(format!("{}/v4/file-urls/batch", self.base_url))
            .bearer_auth(&self.key)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("V4 获取上传 URL 失败: HTTP {} - {}", status, text.chars().take(300).collect::<String>());
        }

        let data: Value = resp.json().await?;
        if data["code"].as_i64().unwrap_or(-1) != 0 {
            anyhow::bail!("V4 获取上传 URL 失败: {}", data["msg"].as_str().unwrap_or("unknown error"));
        }

        let batch_id = data["data"]["batch_id"].as_str()
            .ok_or_else(|| anyhow::anyhow!("V4 响应缺少 batch_id"))?
            .to_string();

        let file_urls = data["data"]["file_urls"].as_array()
            .ok_or_else(|| anyhow::anyhow!("V4 响应缺少 file_urls"))?;

        if file_urls.is_empty() {
            anyhow::bail!("V4 响应 file_urls 为空");
        }

        let upload_url = file_urls[0].as_str()
            .ok_or_else(|| anyhow::anyhow!("V4 file_urls[0] 不是字符串"))?;

        // Step 2: 上传文件到预签名 URL
        // 阿里云 OSS 预签名 URL 的签名不含 Content-Type，需移除默认 Content-Type header
        let upload_resp = self
            .http
            .put(upload_url)
            .header("Content-Type", "")
            .body(pdf_bytes)
            .send()
            .await?;

        if !upload_resp.status().is_success() {
            anyhow::bail!("V4 文件上传失败: HTTP {}", upload_resp.status());
        }

        tracing::info!("V4 文件上传成功, batch_id={}", batch_id);

        // Step 3: 提交解析任务到官方 API
        // POST /api/v4/extract/task/batch 提交批次文件进行解析
        // 请求体包含 batch_id 和 files 列表（含 OSS 文件 URL）
        let file_oss_url = upload_url.split('?').next().unwrap_or(upload_url);
        let submit_body = json!({
            "batch_id": batch_id,
            "files": [{
                "data_id": data_id,
                "url": file_oss_url,
                "name": file_name,
            }],
        });

        tracing::debug!("V4 提交解析任务: {}", submit_body.to_string());

        let submit_resp = self
            .http
            .post(format!("{}/v4/extract/task/batch", self.base_url))
            .bearer_auth(&self.key)
            .json(&submit_body)
            .send()
            .await?;

        if !submit_resp.status().is_success() {
            let status = submit_resp.status();
            let text = submit_resp.text().await.unwrap_or_default();
            anyhow::bail!("V4 提交解析任务失败: HTTP {} - {}", status, text.chars().take(300).collect::<String>());
        }

        let submit_data: Value = submit_resp.json().await?;
        if submit_data["code"].as_i64().unwrap_or(-1) != 0 {
            anyhow::bail!("V4 提交解析任务失败: {}", submit_data["msg"].as_str().unwrap_or("unknown error"));
        }

        let submit_batch_id = submit_data["data"]["batch_id"].as_str()
            .ok_or_else(|| anyhow::anyhow!("V4 提交响应缺少 batch_id"))?
            .to_string();

        tracing::info!("V4 解析任务已提交, batch_id={}, submit_batch_id={}", batch_id, submit_batch_id);

        // 返回 submit_batch_id 用于轮询任务状态
        Ok(submit_batch_id)
    }

    /// V4: 检查任务状态
    /// 使用 batch_id 查询 batch 中所有 task 的状态
    async fn check_task_status_v4(&self, batch_id: &str) -> Value {
        // V4 batch 任务查询：GET /api/v4/extract/task/batch?batch_id={batch_id}
        // 返回 batch 中所有 task 的状态
        let batch_query_url = format!("{}/v4/extract/task/batch?batch_id={}", self.base_url, batch_id);
        let resp = self.http.get(&batch_query_url)
            .bearer_auth(&self.key)
            .send()
            .await;

        if let Ok(resp) = resp {
            if resp.status().is_success() {
                let data: Value = resp.json().await.unwrap_or_default();
                if data["code"].as_i64().unwrap_or(-1) == 0 {
                    if let Some(tasks) = data["data"]["tasks"].as_array() {
                        if tasks.is_empty() {
                            return json!({"_done": false, "status": "pending", "batch_id": batch_id});
                        }

                        // 检查所有 task 的状态
                        let all_done = tasks.iter().all(|t| {
                            t["state"].as_str() == Some("done") || t["state"].as_str() == Some("failed")
                        });
                        let any_failed = tasks.iter().any(|t| t["state"].as_str() == Some("failed"));
                        let any_running = tasks.iter().any(|t| {
                            matches!(t["state"].as_str(), Some("running") | Some("extract") | Some("converting"))
                        });

                        if all_done && !any_failed {
                            // 全部完成且无失败
                            let first_done = tasks.iter().find(|t| t["state"].as_str() == Some("done"));
                            let zip_url = first_done.and_then(|t| t["full_zip_url"].as_str());
                            return json!({
                                "_done": true,
                                "status": "completed",
                                "batch_id": batch_id,
                                "full_zip_url": zip_url,
                            });
                        } else if all_done && any_failed {
                            // 全部结束但有失败
                            let errors: Vec<String> = tasks.iter()
                                .filter_map(|t| t["err_msg"].as_str().map(|s| s.to_string()))
                                .collect();
                            return json!({
                                "_done": true,
                                "status": "failed",
                                "error": errors.join("; "),
                                "batch_id": batch_id,
                            });
                        } else if any_running {
                            return json!({
                                "_done": false,
                                "status": "processing",
                                "batch_id": batch_id,
                                "task_count": tasks.len(),
                            });
                        } else {
                            // 没有运行中的任务，也没有完成的，可能还在排队
                            return json!({
                                "_done": false,
                                "status": "pending",
                                "batch_id": batch_id,
                            });
                        }
                    } else {
                        // response 有 data 但无 tasks 字段，可能 batch 还在处理中
                        return json!({"_done": false, "status": "pending", "batch_id": batch_id});
                    }
                }
            }
        }

        // batch 查询失败，返回等待状态
        json!({
            "_done": false,
            "status": "pending",
            "batch_id": batch_id,
        })
    }

    /// V4: 获取任务结果
    /// 从 status result 中提取 full_zip_url 并下载
    async fn get_task_result_v4(
        &self,
        batch_id: &str,
        output_dir: &str,
        doc_id: Option<i64>,
    ) -> anyhow::Result<String> {
        std::fs::create_dir_all(output_dir)?;

        // 先查询 batch 状态获取 zip_url
        let status = self.check_task_status_v4(batch_id).await;

        // 如果有 full_zip_url 则直接下载
        if let Some(zip_url) = status["full_zip_url"].as_str() {
            return self.v4_download_and_extract_zip(zip_url, output_dir, doc_id).await;
        }

        if status["status"].as_str() == Some("failed") {
            let err = status["error"].as_str().unwrap_or("未知错误");
            anyhow::bail!("V4 解析失败: {}", err);
        }

        // 仍然没有结果
        anyhow::bail!("V4 任务尚未完成 (batch_id={})", batch_id);
    }

    /// V4: 设置完成状态（供外部调用，当轮询找到完成的 task 时设置）
    /// 这个函数被 v4_inject_task_result 使用
    async fn v4_download_and_extract_zip(
        &self,
        zip_url: &str,
        output_dir: &str,
        doc_id: Option<i64>,
    ) -> anyhow::Result<String> {
        // 从 CDN 下载 ZIP
        let resp = self.http.get(zip_url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("下载 ZIP 失败: HTTP {}", resp.status());
        }
        let zip_bytes = resp.bytes().await?.to_vec();

        // 从 ZIP 提取 markdown 和图片
        let (final_markdown, extracted_images) = extract_from_zip(&zip_bytes, output_dir);

        let md = final_markdown.ok_or_else(|| anyhow::anyhow!("ZIP 中未找到 markdown 文件"))?;

        if !extracted_images.is_empty() {
            save_images_zip(output_dir, &extracted_images);
        }

        let md_filename = match doc_id {
            Some(id) => format!("{}.md", id),
            None => "output.md".to_string(),
        };
        let md_path = Path::new(output_dir).join(&md_filename);
        std::fs::write(&md_path, &md)?;
        Ok(md_path.to_string_lossy().to_string())
    }

    /// V4: 直接通过 task_id 查询任务状态（用于轮询找到 task_id 后的查询）
    pub async fn v4_query_task(&self, task_id: &str) -> Value {
        let resp = self
            .http
            .get(format!("{}/v4/extract/task/{}", self.base_url, task_id))
            .bearer_auth(&self.key)
            .send()
            .await;

        match resp {
            Ok(resp) if resp.status().is_success() => {
                let data: Value = resp.json().await.unwrap_or_default();
                if data["code"].as_i64().unwrap_or(-1) != 0 {
                    return json!({"_done": false, "status": "unknown"});
                }
                let state = data["data"]["state"].as_str().unwrap_or("unknown");
                let normalized = match state {
                    "done" => "completed",
                    "failed" => "failed",
                    "pending" | "queued" => "pending",
                    "running" | "extract" | "converting" => "processing",
                    _ => state,
                };
                let is_done = state == "done" || state == "failed";
                json!({
                    "_done": is_done,
                    "status": normalized,
                    "full_zip_url": data["data"].get("full_zip_url"),
                    "error": data["data"].get("err_msg").and_then(|v| v.as_str()).filter(|s| !s.is_empty()),
                    "task_id": task_id,
                })
            }
            _ => json!({"_done": false, "status": "unknown"}),
        }
    }

    /// V4: 通过 task_id 下载结果
    pub async fn v4_get_result_by_task(
        &self,
        task_id: &str,
        output_dir: &str,
        doc_id: Option<i64>,
    ) -> anyhow::Result<String> {
        let status = self.v4_query_task(task_id).await;
        let zip_url = status["full_zip_url"].as_str()
            .ok_or_else(|| anyhow::anyhow!("任务未完成或无 ZIP 下载链接"))?;
        self.v4_download_and_extract_zip(zip_url, output_dir, doc_id).await
    }

    // ════════════════════════════════════════════════════════
    //  实现：V1 Agent 轻量解析 API（type=official-lightweight）
    // ════════════════════════════════════════════════════════

    /// 轻量 API: 上传文件并提交解析
    async fn submit_parse_task_lightweight(&self, file_path: &str) -> anyhow::Result<String> {
        let pdf_bytes = tokio::fs::read(file_path).await?;
        let file_name = Path::new(file_path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "document.pdf".to_string());

        // 使用 multipart 上传文件
        let part = reqwest::multipart::Part::bytes(pdf_bytes)
            .file_name(file_name)
            .mime_str("application/pdf")
            .map_err(|e| anyhow::anyhow!("创建 multipart 失败: {}", e))?;

        let form = reqwest::multipart::Form::new()
            .part("file", part);

        let resp = self
            .http
            .post(format!("{}/v1/agent/parse/file", self.base_url))
            .multipart(form)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("轻量 API 上传失败: HTTP {} - {}", status, text.chars().take(300).collect::<String>());
        }

        let data: Value = resp.json().await?;
        if data["code"].as_i64().unwrap_or(-1) != 0 {
            anyhow::bail!("轻量 API 提交失败: {}", data["msg"].as_str().unwrap_or("unknown error"));
        }

        let task_id = data["data"]["task_id"].as_str()
            .ok_or_else(|| anyhow::anyhow!("轻量 API 响应缺少 task_id"))?
            .to_string();

        tracing::info!("轻量 API 任务已提交: task_id={}", task_id);
        Ok(task_id)
    }

    /// 轻量 API: 检查任务状态
    async fn check_task_status_lightweight(&self, task_id: &str) -> Value {
        let resp = self
            .http
            .get(format!("{}/v1/agent/parse/{}", self.base_url, task_id))
            .send()
            .await;

        match resp {
            Ok(resp) if resp.status().is_success() => {
                let data: Value = resp.json().await.unwrap_or_default();
                if data["code"].as_i64().unwrap_or(-1) != 0 {
                    return json!({"_done": false, "status": "unknown"});
                }
                let state = data["data"]["state"].as_str().unwrap_or("unknown");
                let normalized = match state {
                    "done" => "completed",
                    "failed" => "failed",
                    "pending" | "queued" => "pending",
                    "running" | "extracting" => "processing",
                    _ => state,
                };
                json!({
                    "_done": state == "done" || state == "failed",
                    "status": normalized,
                    "markdown_url": data["data"].get("markdown_url").or_else(|| data["data"].get("url")),
                    "error": data["data"].get("err_msg").and_then(|v| v.as_str()).filter(|s| !s.is_empty()),
                    "task_id": task_id,
                })
            }
            _ => json!({"_done": false, "status": "unknown"}),
        }
    }

    /// 轻量 API: 获取任务结果
    async fn get_task_result_lightweight(
        &self,
        task_id: &str,
        output_dir: &str,
        doc_id: Option<i64>,
    ) -> anyhow::Result<String> {
        let status = self.check_task_status_lightweight(task_id).await;
        if status["status"].as_str() != Some("completed") {
            anyhow::bail!("轻量 API 任务未完成 (status: {})", status["status"].as_str().unwrap_or("unknown"));
        }

        // 获取 markdown URL
        let md_url = status["markdown_url"].as_str()
            .or_else(|| {
                // 也尝试从 data 中获取 url
                status.get("data")
                    .and_then(|d| d.get("url"))
                    .and_then(|v| v.as_str())
            })
            .ok_or_else(|| anyhow::anyhow!("轻量 API 结果缺少 markdown_url"))?;

        // 下载 markdown
        let resp = self.http.get(md_url).send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("下载 markdown 失败: HTTP {}", resp.status());
        }
        let md_content = resp.text().await?;

        std::fs::create_dir_all(output_dir)?;

        let md_filename = match doc_id {
            Some(id) => format!("{}.md", id),
            None => "output.md".to_string(),
        };
        let md_path = Path::new(output_dir).join(&md_filename);
        std::fs::write(&md_path, &md_content)?;

        Ok(md_path.to_string_lossy().to_string())
    }
}

// ════════════════════════════════════════════════════════
//  便捷函数
// ════════════════════════════════════════════════════════

/// 解析 PDF（提交 → 轮询 → 获取结果）
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

/// 使用第一个启用的 MinerU 配置解析 PDF
pub async fn parse_pdf_with_fallback(
    configs: &[MinerUConfig],
    file_path: &str,
    output_dir: &str,
    doc_id: Option<i64>,
) -> anyhow::Result<(usize, String)> {
    let config = configs.first()
        .ok_or_else(|| anyhow::anyhow!("没有可用的 MinerU 配置"))?;
    let client = MinerUClient::new(config);
    let path = parse_pdf(&client, file_path, output_dir, doc_id, config.task_timeout).await?;
    Ok((0, path))
}

// ════════════════════════════════════════════════════════
//  通用工具函数
// ════════════════════════════════════════════════════════

/// 从 ZIP 包中提取 markdown 和图片文件
fn extract_from_zip(zip_data: &[u8], output_dir: &str) -> (Option<String>, Vec<(String, Vec<u8>)>) {
    let mut markdown_content: Option<String> = None;
    let mut image_bytes: Vec<(String, Vec<u8>)> = Vec::new();

    if let Ok(mut archive) = zip::ZipArchive::new(Cursor::new(zip_data)) {
        for i in 0..archive.len() {
            if let Ok(mut entry) = archive.by_index(i) {
                let name = entry.name().to_string();
                let lower = name.to_lowercase();
                let fname = Path::new(&name)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| name.clone());

                if lower.ends_with(".md") || lower.ends_with(".markdown") {
                    let mut buf = Vec::new();
                    if entry.read_to_end(&mut buf).is_ok() {
                        markdown_content = Some(String::from_utf8_lossy(&buf).to_string());
                    }
                } else if lower.ends_with(".png") || lower.ends_with(".jpg") || lower.ends_with(".jpeg")
                    || lower.ends_with(".gif") || lower.ends_with(".svg") || lower.ends_with(".webp")
                    || lower.ends_with(".bmp")
                {
                    let mut buf = Vec::new();
                    if entry.read_to_end(&mut buf).is_ok() {
                        let img_path = Path::new(output_dir).join(&fname);
                        if let Some(parent) = img_path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::write(&img_path, &buf);
                        image_bytes.push((fname, buf));
                    }
                }
            }
        }
    }

    (markdown_content, image_bytes)
}

/// 将图片列表打包为 images.zip
fn save_images_zip(output_dir: &str, images: &[(String, Vec<u8>)]) {
    let images_zip_path = Path::new(output_dir).join("images.zip");
    match std::fs::File::create(&images_zip_path) {
        Ok(zip_file) => {
            let mut zip_writer = zip::ZipWriter::new(zip_file);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (filename, data) in images {
                if zip_writer.start_file(filename, options).is_ok() {
                    let _ = zip_writer.write_all(data);
                }
            }
            let _ = zip_writer.finish();
        }
        Err(e) => {
            tracing::warn!("无法创建 images.zip: {}", e);
        }
    }
}

/// 解析 URL：将相对路径拼接到 base_url 上
fn resolve_url(base: &str, url: &str) -> String {
    if url.starts_with("http://") || url.starts_with("https://") {
        url.to_string()
    } else if url.starts_with('/') {
        format!("{}{}", base.trim_end_matches('/'), url)
    } else {
        format!("{}/{}", base.trim_end_matches('/'), url)
    }
}

/// 判断两个 URL 是否同源
fn is_same_origin(url_a: &str, url_b: &str) -> bool {
    let a_parsed = url::Url::parse(url_a);
    let b_parsed = url::Url::parse(url_b);
    match (a_parsed, b_parsed) {
        (Ok(a), Ok(b)) => a.origin() == b.origin(),
        _ => false,
    }
}