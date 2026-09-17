//! MinerU PDF 解析服务。

use std::io::{Cursor, Read, Write};
use std::path::Path;

use serde_json::{json, Value};

pub struct MinerUClient {
    base_url: String,
    key: String,
    http: reqwest::Client,
}

impl MinerUClient {
    pub fn new(base_url: &str, key: &str) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            key: key.to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// 提交 PDF 解析任务
    pub async fn submit_parse_task(&self, file_path: &str) -> anyhow::Result<String> {
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

    /// 检查任务状态
    pub async fn check_task_status(&self, task_id: &str) -> Value {
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

    /// 轮询任务直到完成
    pub async fn poll_task(&self, task_id: &str, timeout_secs: u64) -> anyhow::Result<Value> {
        let poll_interval = 2u64;
        let max_polls = timeout_secs / poll_interval;

        for _ in 0..max_polls {
            let status_result = self.check_task_status(task_id).await;
            let status = status_result["status"].as_str().unwrap_or("unknown");

            match status {
                "completed" => return Ok(status_result),
                "failed" => {
                    let error = status_result["error"].as_str().unwrap_or("Unknown error");
                    anyhow::bail!("MinerU task failed: {}", error);
                }
                _ => {
                    tokio::time::sleep(std::time::Duration::from_secs(poll_interval)).await;
                }
            }
        }

        anyhow::bail!("MinerU task timed out: {}", task_id);
    }

    /// 获取任务结果并保存到输出目录
    pub async fn get_task_result(
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
