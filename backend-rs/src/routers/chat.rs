//! 简易问答（Chat）路由。
//!
//! 复用 `config.json` 的 `llm.models` 模型池：前端可选择池中任一模型，
//! 服务端按所选模型构造 `LlmClient` 并做多轮生成。
//!
//! 开启工具时，模型可调用本系统的 MCP 工具集（`McpServer::list_tools()`，
//! 全部为只读的资料库查询），由服务端在本请求内完成「模型 → 工具 → 模型」
//! 的循环，直到模型给出最终文本回答。
//!
//! 工具调用支持两种协议（`tool_mode`）：
//! - `native`：原生 function calling（Ollama `/api/chat` / OpenAI 兼容 `tools`）
//! - `prompt`：提示词协议（system 里给出工具清单，模型输出 `{"tool_calls":[...]}` JSON）。
//!   部分网关/思考型模型不支持 function calling，只能走这条通道。
//! - `auto`（默认）：先试 native；服务端不支持（报错含 tool/function）或模型该轮
//!   直接说“无法访问文献库”时，自动降级为 prompt。
//!
//! 当前为非流式实现（一次请求返回完整回答），与 `/assist/api/config/test`
//! 一样属于前台同步操作，超时由前端负责放大。

use std::collections::HashMap;
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Json, Response};
use axum::routing::{get, post};
use axum::Extension;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::auth;
use crate::config::Settings;
use crate::database::Database;
use crate::mcp_server::McpServer;
use crate::services::llm::{parse_text_tool_calls, text_tool_prompt, ChatMessage, LlmClient, ToolCall, ToolMode};

/// 允许使用问答功能的角色（只读角色也可提问）
const CHAT_ROLES: &[&str] = &["admin", "view-only", "view-upload"];

/// 未显式传入 system 时使用的默认系统提示词
const DEFAULT_SYSTEM_PROMPT: &str = "你是一个严谨的学术助手，回答使用与提问相同的语言，\
尽量给出结构化、可核查的说明，不确定时明确说明不确定。";

/// 启用工具时追加的系统提示词（原生 function calling 通道）
const TOOL_SYSTEM_PROMPT: &str = "\
你可以调用工具检索本地的文献库（全部为只读查询）。

调用规则：
1. 问题涉及本地文献库的内容、文献列表、统计、原文或目录时，必须先调用工具获取事实，不要凭印象回答。
2. 找文献先用 search_info（题录：题目/作者/期刊/摘要）或 grep_search（全文），需要细节再用 read_markdown 或 get_toc；用户给出 ID 时直接用 id 参数。
3. 结论标注来源，格式如 [id=12]；引用原文时保持原样。
4. 工具返回空结果或报错时如实说明，不要编造文献或数据。
5. 一次可以并行请求多个互不依赖的工具调用；信息足够时立即给出最终回答。";

/// 模型声称“自己没法查文献库”时的降级信号（仅在 auto 模式首轮生效）
const REFUSAL_HINTS: &[&str] = &[
    "无法",
    "不能",
    "没法",
    "找不到",
    "没有权限",
    "无法访问",
    "不具备",
    "cannot",
    "can't",
    "unable",
    "no access",
    "don't have access",
    "do not have access",
    "not able to",
    "without access",
];

/// 单次回答的最大 token 数上限（防止误传超大值）
const MAX_TOKENS_LIMIT: u32 = 8192;
/// 一轮问答内最多执行的「模型 → 工具」轮数
const MAX_TOOL_ROUNDS: usize = 5;
/// 单个工具结果写入上下文时的最大字符数
const TOOL_RESULT_LIMIT: usize = 12000;
/// 返回给前端展示的工具结果预览长度
const TOOL_PREVIEW_LIMIT: usize = 400;

pub fn router() -> axum::Router<()> {
    axum::Router::new()
        .route("/assist/api/chat/models/", get(list_models))
        .route("/assist/api/chat/models", get(list_models))
        .route("/assist/api/chat/tools/", get(list_tools))
        .route("/assist/api/chat/tools", get(list_tools))
        .route("/assist/api/chat/", post(chat))
        .route("/assist/api/chat", post(chat))
}

#[derive(Deserialize)]
struct ChatRequest {
    /// 模型池中的模型名（留空或未命中取第一项）
    #[serde(default)]
    model: Option<String>,
    messages: Vec<ChatMessage>,
    /// 覆盖默认系统提示词；为空则不额外添加（除非 messages 里已有 system）
    #[serde(default)]
    system: Option<String>,
    #[serde(default)]
    temperature: Option<f64>,
    #[serde(default)]
    max_tokens: Option<u32>,
    /// 是否允许模型调用本系统 MCP 工具（默认开启）
    #[serde(default = "default_true")]
    tools: bool,
    /// 工具调用方式覆盖：auto / native / prompt（默认取模型配置，再回退 auto）
    #[serde(default)]
    tool_mode: Option<String>,
    /// 一轮内最多执行的工具轮数（默认 5，上限 8）
    #[serde(default)]
    max_tool_rounds: Option<usize>,
}

fn default_true() -> bool {
    true
}

/// `GET /assist/api/chat/models` —— 模型池列表（不含 key 等敏感字段）
async fn list_models(
    Extension(role): Extension<auth::Role>,
    Extension(settings): Extension<Arc<Settings>>,
) -> Response {
    if let Err(resp) = auth::assert_role(&role, CHAT_ROLES) {
        return resp;
    }

    let models: Vec<Value> = settings
        .llm_models()
        .iter()
        .map(|m| {
            json!({
                "name": m.name,
                "protocol": m.protocol,
                "model": m.model,
                "tool_mode": ToolMode::parse(&m.tool_mode).as_str(),
            })
        })
        .collect();
    let default_name = settings.llm_model_config(None).name;

    Json(json!({
        "models": models,
        "count": models.len(),
        "default": default_name,
    }))
    .into_response()
}

/// `GET /assist/api/chat/tools` —— 可被模型调用的 MCP 工具清单
async fn list_tools(
    Extension(role): Extension<auth::Role>,
    Extension(settings): Extension<Arc<Settings>>,
    Extension(db): Extension<Arc<Database>>,
) -> Response {
    if let Err(resp) = auth::assert_role(&role, CHAT_ROLES) {
        return resp;
    }

    let mcp = McpServer::new(db, settings);
    let tools = mcp.list_tools().await;
    let names: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "name": t["name"],
                "description": t["description"],
            })
        })
        .collect();

    Json(json!({
        "count": names.len(),
        "tools": names,
    }))
    .into_response()
}

/// `POST /assist/api/chat` —— 多轮对话生成（非流式，可选工具调用）
async fn chat(
    Extension(role): Extension<auth::Role>,
    Extension(settings): Extension<Arc<Settings>>,
    Extension(db): Extension<Arc<Database>>,
    Json(body): Json<ChatRequest>,
) -> Response {
    if let Err(resp) = auth::assert_role(&role, CHAT_ROLES) {
        return resp;
    }

    // 角色白名单校验：system / user / assistant / tool
    if body.messages.iter().any(|m| !m.is_valid_role()) {
        return bad_request("消息 role 只能是 system / user / assistant / tool");
    }

    // 去掉空内容消息（工具调用消息 content 为空但有 tool_calls，必须保留）
    let mut messages: Vec<ChatMessage> = body
        .messages
        .iter()
        .filter(|m| m.is_meaningful())
        .cloned()
        .collect();
    if messages.is_empty() {
        return bad_request("messages 不能为空");
    }

    let temperature = body.temperature.unwrap_or(0.7).clamp(0.0, 2.0);
    let max_tokens = body.max_tokens.unwrap_or(2048).clamp(1, MAX_TOKENS_LIMIT);
    let max_tool_rounds = body.max_tool_rounds.unwrap_or(MAX_TOOL_ROUNDS).min(8);

    let endpoint = settings.llm_model_config(body.model.as_deref());
    let client = endpoint.client();

    // 工具集（本系统 MCP 工具，全部只读）
    let mcp = McpServer::new(db, settings.clone());
    let tools: Vec<Value> = if body.tools && max_tool_rounds > 0 {
        mcp.list_tools().await
    } else {
        Vec::new()
    };

    // 工具调用方式：请求参数 > 模型配置 > auto
    let mode = body
        .tool_mode
        .as_deref()
        .map(ToolMode::parse)
        .unwrap_or_else(|| ToolMode::parse(&endpoint.tool_mode));

    // 补系统提示词（已有 system 消息时不重复添加）
    let has_system = messages.iter().any(|m| m.role == "system");
    if !has_system {
        let base = body
            .system
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_SYSTEM_PROMPT);
        let system = if tools.is_empty() {
            base.to_string()
        } else {
            format!("{}\n\n{}", base, TOOL_SYSTEM_PROMPT)
        };
        messages.insert(0, ChatMessage::new("system", &system));
    }

    let turn = match run_turn(
        &client,
        &mcp,
        messages,
        &tools,
        mode,
        temperature,
        max_tokens,
        max_tool_rounds,
    )
    .await
    {
        Ok(t) => t,
        Err(e) => return bad_gateway(e.0),
    };

    if turn.content.trim().is_empty() {
        return bad_gateway("模型返回了空响应".to_string());
    }

    Json(json!({
        "content": turn.content,
        "steps": turn.steps,
        "delta": turn.delta,
        "notes": turn.notes,
        "name": endpoint.name,
        "protocol": endpoint.protocol,
        "model": endpoint.model,
        "tool_mode": turn.mode.as_str(),
        "tool_rounds": turn.rounds,
    }))
    .into_response()
}

// ── 一轮问答（含工具循环）────────────────────────────────

/// 工具协议降级原因（`auto` 模式下由 native 通道返回）
struct FallbackReason(String);

/// 一轮问答的结果
struct TurnOutcome {
    content: String,
    steps: Vec<Value>,
    delta: Vec<ChatMessage>,
    notes: Vec<String>,
    /// 实际使用的协议
    mode: ToolMode,
    rounds: usize,
}

/// `FallbackReason` → 面向用户的错误文本
impl From<FallbackReason> for String {
    fn from(reason: FallbackReason) -> Self {
        reason.0
    }
}

/// 执行一轮问答：无工具时直接生成；有工具时按 `mode` 走 native / prompt 循环
#[allow(clippy::too_many_arguments)]
async fn run_turn(
    client: &LlmClient,
    mcp: &McpServer,
    messages: Vec<ChatMessage>,
    tools: &[Value],
    mode: ToolMode,
    temperature: f64,
    max_tokens: u32,
    max_rounds: usize,
) -> Result<TurnOutcome, FallbackReason> {
    // 未启用工具：普通对话
    if tools.is_empty() {
        let reply = client
            .chat_with_tools(&messages, &[], temperature, max_tokens)
            .await
            .map_err(|e| FallbackReason(format!("模型调用失败: {}", e)))?;
        let mut delta = Vec::new();
        let content = ensure_text(&reply);
        delta.push(ChatMessage::new("assistant", &content));
        return Ok(TurnOutcome {
            content,
            steps: Vec::new(),
            delta,
            notes: Vec::new(),
            mode: ToolMode::Native,
            rounds: 0,
        });
    }

    match mode {
        ToolMode::Prompt => {
            run_turn_prompt(client, mcp, messages, tools, temperature, max_tokens, max_rounds).await
        }
        ToolMode::Native => {
            run_turn_native(client, mcp, messages, tools, temperature, max_tokens, max_rounds).await
        }
        ToolMode::Auto => {
            // 先试原生 function calling
            match run_turn_native(
                client,
                mcp,
                messages.clone(),
                tools,
                temperature,
                max_tokens,
                max_rounds,
            )
            .await
            {
                Ok(outcome) => Ok(outcome),
                Err(reason) => {
                    // 服务商不支持或模型该轮不动手 → 降级为提示词协议重跑
                    let mut outcome = run_turn_prompt(
                        client, mcp, messages, tools, temperature, max_tokens, max_rounds,
                    )
                    .await?;
                    outcome.mode = ToolMode::Prompt;
                    outcome.notes.insert(
                        0,
                        format!("已从原生 function calling 自动降级为提示词工具协议（{}）", reason.0),
                    );
                    Ok(outcome)
                }
            }
        }
    }
}

/// 原生 function calling 循环
///
/// `Err` 表示“应该改用提示词协议”：服务商报错含 tool/function，或模型首轮
/// 没有发起调用却在回答里声称自己无法访问文献库（/ 返回空内容）。
async fn run_turn_native(
    client: &LlmClient,
    mcp: &McpServer,
    mut messages: Vec<ChatMessage>,
    tools: &[Value],
    temperature: f64,
    max_tokens: u32,
    max_rounds: usize,
) -> Result<TurnOutcome, FallbackReason> {
    let mut delta: Vec<ChatMessage> = Vec::new();
    let mut steps: Vec<Value> = Vec::new();
    let mut cached: HashMap<(String, String), String> = HashMap::new();
    let mut rounds = 0usize;

    let content = loop {
        let expose = rounds < max_rounds;
        let reply = match client
            .chat_with_tools(
                &messages,
                if expose { tools } else { &[] },
                temperature,
                max_tokens,
            )
            .await
        {
            Ok(r) => r,
            Err(e) => {
                let msg = e.to_string();
                let lower = msg.to_lowercase();
                // 服务商不认 tools/function 参数 → 交给提示词协议
                if lower.contains("tool") || lower.contains("function") {
                    return Err(FallbackReason(format!("服务商拒绝原生 tools 参数：{}", msg)));
                }
                return Err(FallbackReason(format!("模型调用失败: {}", msg)));
            }
        };

        if reply.tool_calls.is_empty() {
            // 首轮就宣称“我查不了文献库” → 说明该模型不会用原生工具，降级重试
            if rounds == 0
                && (reply.content.trim().is_empty() || looks_like_refusal(&reply.content))
            {
                return Err(FallbackReason(
                    "模型未发起工具调用（可能不支持 function calling）".to_string(),
                ));
            }
            let text = ensure_text(&reply);
            messages.push(ChatMessage::new("assistant", &text));
            delta.push(ChatMessage::new("assistant", &text));
            break text;
        }

        // 轮数用尽但模型仍在请求工具：插入提示让它基于已有信息收尾
        if !expose {
            messages.push(ChatMessage::new(
                "system",
                "工具调用轮数已达上限，请仅基于已获得的信息作答，不要再请求工具。",
            ));
            let reply = client
                .chat_with_tools(&messages, &[], temperature, max_tokens)
                .await
                .map_err(|e| FallbackReason(format!("模型调用失败: {}", e)))?;
            let text = ensure_text(&reply);
            messages.push(ChatMessage::new("assistant", &text));
            delta.push(ChatMessage::new("assistant", &text));
            break text;
        }

        rounds += 1;
        let assistant_msg = ChatMessage::assistant_tool_calls(&reply.content, reply.tool_calls.clone());
        messages.push(assistant_msg.clone());
        delta.push(assistant_msg);

        for result in execute_calls(mcp, &reply.tool_calls, rounds, &mut cached, &mut steps).await {
            let tool_msg = ChatMessage::tool_result(&result.call, &result.text);
            messages.push(tool_msg.clone());
            delta.push(tool_msg);
        }
    };

    Ok(TurnOutcome {
        content,
        steps,
        delta,
        notes: Vec::new(),
        mode: ToolMode::Native,
        rounds,
    })
}

/// 提示词工具协议循环
///
/// 不支持 function calling 的模型只能按 system 提示词输出 JSON 工具调用，
/// 因此工具结果以 `user` 消息形式回传（模型不认识 `tool` 角色）。
async fn run_turn_prompt(
    client: &LlmClient,
    mcp: &McpServer,
    mut messages: Vec<ChatMessage>,
    tools: &[Value],
    temperature: f64,
    max_tokens: u32,
    max_rounds: usize,
) -> Result<TurnOutcome, FallbackReason> {
    // 把工具清单与调用格式追加进 system 提示词
    let tool_prompt = text_tool_prompt(tools);
    match messages.iter().position(|m| m.role == "system") {
        Some(i) => messages[i].content = format!("{}\n\n{}", messages[i].content, tool_prompt),
        None => messages.insert(0, ChatMessage::new("system", &tool_prompt)),
    }

    let mut delta: Vec<ChatMessage> = Vec::new();
    let mut steps: Vec<Value> = Vec::new();
    let mut cached: HashMap<(String, String), String> = HashMap::new();
    let mut rounds = 0usize;

    let content = loop {
        let reply = client
            .chat_with_tools(&messages, &[], temperature, max_tokens)
            .await
            .map_err(|e| FallbackReason(format!("模型调用失败: {}", e)))?;

        let calls = parse_text_tool_calls(&reply.content);
        if calls.is_empty() || rounds >= max_rounds {
            let text = ensure_text(&reply);
            messages.push(ChatMessage::new("assistant", &text));
            delta.push(ChatMessage::new("assistant", &text));
            break text;
        }

        rounds += 1;
        // 保留模型自己的 JSON 输出，让它知道刚才请求了什么
        let assistant_msg = ChatMessage::assistant_tool_calls(&reply.content, calls.clone());
        messages.push(assistant_msg.clone());
        delta.push(assistant_msg);

        for result in execute_calls(mcp, &calls, rounds, &mut cached, &mut steps).await {
            let tool_msg = ChatMessage::new(
                "user",
                &format!(
                    "【工具结果】{} 调用完成，返回如下（已截断超长内容）：\n{}",
                    result.call.name, result.text
                ),
            );
            messages.push(tool_msg.clone());
            delta.push(tool_msg);
        }
    };

    Ok(TurnOutcome {
        content,
        steps,
        delta,
        notes: Vec::new(),
        mode: ToolMode::Prompt,
        rounds,
    })
}

/// 一次工具调用及其结果
struct CallResult {
    call: ToolCall,
    text: String,
}

/// 顺序执行工具调用（相同调用复用结果），并记录前端展示用的步骤
async fn execute_calls(
    mcp: &McpServer,
    calls: &[ToolCall],
    round: usize,
    cached: &mut HashMap<(String, String), String>,
    steps: &mut Vec<Value>,
) -> Vec<CallResult> {
    let mut results = Vec::with_capacity(calls.len());
    for call in calls {
        let key = (call.name.clone(), call.arguments.to_string());
        let (text, is_error) = match cached.get(&key) {
            Some(hit) => (hit.clone(), false),
            None => {
                let value = mcp.call_tool(&call.name, &call.arguments).await;
                let (text, is_error) = tool_result_text(&value);
                cached.insert(key, text.clone());
                (text, is_error)
            }
        };

        steps.push(json!({
            "round": round,
            "name": call.name,
            "arguments": call.arguments,
            "is_error": is_error,
            "preview": truncate_chars(&text, TOOL_PREVIEW_LIMIT),
            "chars": text.chars().count(),
        }));

        results.push(CallResult {
            call: call.clone(),
            text: truncate_chars(&text, TOOL_RESULT_LIMIT),
        });
    }
    results
}

/// 取最终文本；为空时给出可诊断的错误（区分被 max_tokens 截断的情形）
///
/// 注意：调用方负责把最终 assistant 消息 push 进 `delta`（用于跨轮续接上下文）。
fn ensure_text(reply: &crate::services::llm::ChatReply) -> String {
    if !reply.content.trim().is_empty() {
        return reply.content.clone();
    }
    if let Some(reason) = reply.finish_reason.as_deref() {
        if reason == "length" || reason == "max_tokens" {
            return "（模型输出在给出正式回答前被 max_tokens 截断：该模型会先输出思考内容，\
请增大 max_tokens 或简化问题后重试）"
                .to_string();
        }
        return format!("（模型没有返回文本内容，finish_reason={}）", reason);
    }
    "（模型没有返回文本内容）".to_string()
}

/// 模型是否在表示自己“无法访问文献库”（用于 auto 模式降级判断）
fn looks_like_refusal(text: &str) -> bool {
    let lower = text.to_lowercase();
    REFUSAL_HINTS.iter().any(|h| lower.contains(h))
}

/// 从 `McpServer::call_tool` 的返回值中取出文本结果
///
/// 形态为 `{"content":[{"type":"text","text":"..."}]}`，出错时为 `{"isError":true,...}`。
fn tool_result_text(value: &Value) -> (String, bool) {
    let is_error = value["isError"].as_bool().unwrap_or(false);
    let text = value["content"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default();
    if text.is_empty() && is_error {
        ("工具执行失败（无详细信息）".to_string(), true)
    } else {
        (text, is_error)
    }
}

/// 按字符数截断（避免把超大工具结果整体塞进上下文/前端）
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{}\n…（内容过长已截断）", head)
}

fn bad_request(detail: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"detail": detail}))).into_response()
}

fn bad_gateway(detail: String) -> Response {
    (StatusCode::BAD_GATEWAY, Json(json!({"detail": detail}))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refusal_detection_matches_capability_claims() {
        assert!(looks_like_refusal("我无法在知识库中搜索文章。"));
        assert!(looks_like_refusal("I cannot search the knowledge base."));
        assert!(looks_like_refusal("I don't have access to your library."));
        assert!(!looks_like_refusal("钠离子电池的综述见 [id=12]。"));
    }

    #[test]
    fn tool_result_text_handles_mcp_shapes() {
        let (text, err) = tool_result_text(&json!({"content": [{"type": "text", "text": "总数 3"}]}));
        assert_eq!(text, "总数 3");
        assert!(!err);

        let (text, err) = tool_result_text(&json!({
            "isError": true,
            "content": [{"type": "text", "text": "缺少必需参数: id"}]
        }));
        assert_eq!(text, "缺少必需参数: id");
        assert!(err);

        let (text, err) = tool_result_text(&json!({"isError": true}));
        assert!(text.contains("工具执行失败"));
        assert!(err);
    }

    #[test]
    fn truncate_chars_keeps_short_text_intact() {
        assert_eq!(truncate_chars("abc", 10), "abc");
        assert!(truncate_chars("abcdef", 3).starts_with("abc"));
        assert!(truncate_chars("abcdef", 3).contains("已截断"));
    }
}
