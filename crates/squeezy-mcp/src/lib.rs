use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use rmcp::{
    ServiceExt,
    model::{CallToolRequestParams, JsonObject, Tool as RmcpTool},
    transport::{StreamableHttpClientTransport, TokioChildProcess},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use squeezy_core::{McpServerConfig, McpTransport};
use tokio_util::sync::CancellationToken;

const DEFAULT_MCP_TIMEOUT_MS: u64 = 30_000;

pub type McpResult<T> = Result<T, McpError>;

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("MCP server {server:?} is missing command for stdio transport")]
    MissingCommand { server: String },
    #[error("MCP server {server:?} is missing url for {transport} transport")]
    MissingUrl {
        server: String,
        transport: &'static str,
    },
    #[error("MCP server {server:?} timed out after {timeout_ms}ms")]
    Timeout { server: String, timeout_ms: u64 },
    #[error("MCP server {server:?} call was cancelled")]
    Cancelled { server: String },
    #[error("MCP tool {tool:?} expects object arguments")]
    InvalidArguments { tool: String },
    #[error("unknown MCP tool {tool:?}")]
    UnknownTool { tool: String },
    #[error("MCP server {server:?}: {message}")]
    Transport { server: String, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalMcpTool {
    pub server: String,
    pub raw_name: String,
    pub model_name: String,
    pub description: String,
    pub parameters: Value,
    pub transport: McpTransport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalMcpToolResult {
    pub server: String,
    pub raw_name: String,
    pub model_name: String,
    pub is_error: bool,
    pub content: Value,
}

#[derive(Clone, Default)]
pub struct McpClientRegistry {
    servers: Arc<BTreeMap<String, McpServerConfig>>,
    cache: Arc<Mutex<BTreeMap<String, ExternalMcpTool>>>,
}

impl McpClientRegistry {
    pub fn new(servers: BTreeMap<String, McpServerConfig>) -> Self {
        Self {
            servers: Arc::new(servers),
            cache: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.servers.iter().all(|(_, server)| !server.enabled)
    }

    pub fn tools(&self) -> Vec<ExternalMcpTool> {
        self.cache
            .lock()
            .map(|cache| cache.values().cloned().collect())
            .unwrap_or_default()
    }

    pub fn tool(&self, model_name: &str) -> Option<ExternalMcpTool> {
        self.cache
            .lock()
            .ok()
            .and_then(|cache| cache.get(model_name).cloned())
    }

    pub async fn refresh_tools(&self, cancel: CancellationToken) -> Vec<McpError> {
        if self.is_empty() {
            if let Ok(mut cache) = self.cache.lock() {
                cache.clear();
            }
            return Vec::new();
        }

        let mut next = BTreeMap::new();
        let mut errors = Vec::new();
        for (server_name, server) in self.servers.iter() {
            if !server.enabled {
                continue;
            }
            match discover_server_tools(server_name, server, cancel.clone()).await {
                Ok(tools) => {
                    for tool in tools {
                        let model_name = unique_model_name(&next, &tool.model_name);
                        next.insert(model_name.clone(), ExternalMcpTool { model_name, ..tool });
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        target: "squeezy::mcp",
                        server = %server_name,
                        error = %error,
                        "failed to discover MCP tools"
                    );
                    errors.push(error);
                }
            }
        }
        if let Ok(mut cache) = self.cache.lock() {
            *cache = next;
        }
        errors
    }

    pub async fn call_tool(
        &self,
        model_name: &str,
        arguments: Value,
        cancel: CancellationToken,
    ) -> McpResult<ExternalMcpToolResult> {
        let tool = self.tool(model_name).ok_or_else(|| McpError::UnknownTool {
            tool: model_name.to_string(),
        })?;
        let server = self
            .servers
            .get(&tool.server)
            .ok_or_else(|| McpError::UnknownTool {
                tool: model_name.to_string(),
            })?;
        let args = arguments_object(&tool.model_name, arguments)?;
        let result = call_server_tool(&tool.server, server, &tool.raw_name, args, cancel).await?;
        Ok(ExternalMcpToolResult {
            server: tool.server,
            raw_name: tool.raw_name,
            model_name: tool.model_name,
            is_error: result
                .get("isError")
                .and_then(Value::as_bool)
                .or_else(|| result.get("is_error").and_then(Value::as_bool))
                .unwrap_or(false),
            content: result,
        })
    }
}

async fn discover_server_tools(
    server_name: &str,
    server: &McpServerConfig,
    cancel: CancellationToken,
) -> McpResult<Vec<ExternalMcpTool>> {
    let timeout_ms = server.timeout_ms.unwrap_or(DEFAULT_MCP_TIMEOUT_MS);
    with_timeout(server_name, timeout_ms, cancel, async {
        match server.transport {
            McpTransport::Stdio => discover_stdio_tools(server_name, server).await,
            McpTransport::Http | McpTransport::Sse => {
                discover_http_tools(server_name, server).await
            }
        }
    })
    .await
}

async fn call_server_tool(
    server_name: &str,
    server: &McpServerConfig,
    tool_name: &str,
    arguments: JsonObject,
    cancel: CancellationToken,
) -> McpResult<Value> {
    let timeout_ms = server.timeout_ms.unwrap_or(DEFAULT_MCP_TIMEOUT_MS);
    with_timeout(server_name, timeout_ms, cancel, async {
        match server.transport {
            McpTransport::Stdio => call_stdio_tool(server_name, server, tool_name, arguments).await,
            McpTransport::Http | McpTransport::Sse => {
                call_http_tool(server_name, server, tool_name, arguments).await
            }
        }
    })
    .await
}

async fn discover_stdio_tools(
    server_name: &str,
    server: &McpServerConfig,
) -> McpResult<Vec<ExternalMcpTool>> {
    let service = start_stdio_service(server_name, server).await?;
    let tools = service
        .list_all_tools()
        .await
        .map_err(|err| McpError::Transport {
            server: server_name.to_string(),
            message: err.to_string(),
        })?;
    let _ = service.cancel().await;
    Ok(convert_tools(server_name, server.transport, tools))
}

async fn call_stdio_tool(
    server_name: &str,
    server: &McpServerConfig,
    tool_name: &str,
    arguments: JsonObject,
) -> McpResult<Value> {
    let service = start_stdio_service(server_name, server).await?;
    let result = service
        .call_tool(CallToolRequestParams::new(tool_name.to_string()).with_arguments(arguments))
        .await
        .map_err(|err| McpError::Transport {
            server: server_name.to_string(),
            message: err.to_string(),
        })?;
    let _ = service.cancel().await;
    serde_json::to_value(result).map_err(|err| McpError::Transport {
        server: server_name.to_string(),
        message: err.to_string(),
    })
}

async fn discover_http_tools(
    server_name: &str,
    server: &McpServerConfig,
) -> McpResult<Vec<ExternalMcpTool>> {
    let service = start_http_service(server_name, server).await?;
    let tools = service
        .list_all_tools()
        .await
        .map_err(|err| McpError::Transport {
            server: server_name.to_string(),
            message: err.to_string(),
        })?;
    let _ = service.cancel().await;
    Ok(convert_tools(server_name, server.transport, tools))
}

async fn call_http_tool(
    server_name: &str,
    server: &McpServerConfig,
    tool_name: &str,
    arguments: JsonObject,
) -> McpResult<Value> {
    let service = start_http_service(server_name, server).await?;
    let result = service
        .call_tool(CallToolRequestParams::new(tool_name.to_string()).with_arguments(arguments))
        .await
        .map_err(|err| McpError::Transport {
            server: server_name.to_string(),
            message: err.to_string(),
        })?;
    let _ = service.cancel().await;
    serde_json::to_value(result).map_err(|err| McpError::Transport {
        server: server_name.to_string(),
        message: err.to_string(),
    })
}

async fn start_stdio_service(
    server_name: &str,
    server: &McpServerConfig,
) -> McpResult<rmcp::service::RunningService<rmcp::service::RoleClient, ()>> {
    let command = server
        .command
        .as_ref()
        .ok_or_else(|| McpError::MissingCommand {
            server: server_name.to_string(),
        })?;
    let mut process = tokio::process::Command::new(command);
    process.args(&server.args);
    process.envs(&server.env);
    let transport = TokioChildProcess::new(process).map_err(|err| McpError::Transport {
        server: server_name.to_string(),
        message: err.to_string(),
    })?;
    ().serve(transport)
        .await
        .map_err(|err| McpError::Transport {
            server: server_name.to_string(),
            message: err.to_string(),
        })
}

async fn start_http_service(
    server_name: &str,
    server: &McpServerConfig,
) -> McpResult<rmcp::service::RunningService<rmcp::service::RoleClient, ()>> {
    let url = server.url.as_ref().ok_or_else(|| McpError::MissingUrl {
        server: server_name.to_string(),
        transport: match server.transport {
            McpTransport::Http => "http",
            McpTransport::Sse => "sse",
            McpTransport::Stdio => "stdio",
        },
    })?;
    let transport = StreamableHttpClientTransport::from_uri(url.clone());
    ().serve(transport)
        .await
        .map_err(|err| McpError::Transport {
            server: server_name.to_string(),
            message: err.to_string(),
        })
}

async fn with_timeout<T>(
    server_name: &str,
    timeout_ms: u64,
    cancel: CancellationToken,
    future: impl std::future::Future<Output = McpResult<T>>,
) -> McpResult<T> {
    tokio::select! {
        _ = cancel.cancelled() => Err(McpError::Cancelled { server: server_name.to_string() }),
        result = tokio::time::timeout(Duration::from_millis(timeout_ms), future) => {
            result.map_err(|_| McpError::Timeout {
                server: server_name.to_string(),
                timeout_ms,
            })?
        }
    }
}

fn convert_tools(
    server_name: &str,
    transport: McpTransport,
    tools: Vec<RmcpTool>,
) -> Vec<ExternalMcpTool> {
    tools
        .into_iter()
        .map(|tool| {
            let raw_name = tool.name.to_string();
            let description = tool
                .description
                .as_ref()
                .map(|description| description.to_string())
                .unwrap_or_else(|| format!("MCP tool {server_name}/{raw_name}"));
            let parameters = schema_object(tool.schema_as_json_value());
            ExternalMcpTool {
                server: server_name.to_string(),
                raw_name: raw_name.clone(),
                model_name: external_tool_name(server_name, &raw_name),
                description,
                parameters,
                transport,
            }
        })
        .collect()
}

fn schema_object(value: Value) -> Value {
    if value.as_object().is_some() {
        value
    } else {
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": true,
        })
    }
}

fn arguments_object(tool: &str, arguments: Value) -> McpResult<JsonObject> {
    match arguments {
        Value::Null => Ok(JsonObject::new()),
        Value::Object(map) => Ok(map),
        _ => Err(McpError::InvalidArguments {
            tool: tool.to_string(),
        }),
    }
}

fn external_tool_name(server: &str, tool: &str) -> String {
    format!("mcp__{}__{}", sanitize_name(server), sanitize_name(tool))
}

fn sanitize_name(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('_');
        }
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        "tool".to_string()
    } else {
        out
    }
}

fn unique_model_name(existing: &BTreeMap<String, ExternalMcpTool>, candidate: &str) -> String {
    if !existing.contains_key(candidate) {
        return candidate.to_string();
    }
    for index in 2usize.. {
        let next = format!("{candidate}__{index}");
        if !existing.contains_key(&next) {
            return next;
        }
    }
    unreachable!("unbounded suffix search must find a unique name")
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
