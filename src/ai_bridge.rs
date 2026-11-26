//! 本地 AI 桥：给游戏内「小七」伙伴提供 127.0.0.1 上的 HTTP 接口。
//!
//! - 端口随机绑定、随机 Bearer token（仅注入给本机游戏进程）；
//! - `POST /hello {player}` → `{greeting}` 进世界问候；
//! - `POST /chat {player,message}` → `{reply,action}`，action ∈
//!   follow/stop/come/look/none（模组据此改变跟随行为）；
//! - 聊天决策：本地规则优先（跟随/停下/过来/看我 等关键词）→ 配置了
//!   OpenAI 兼容 API 时走 LLM → 否则给规则提示（不报错，离线可用）。

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// 桥接实例：端口 + token，生命周期跟随启动器进程。
pub struct Bridge {
    pub port: u16,
    pub token: String,
    stop: Arc<AtomicBool>,
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// 启动本地桥（后台线程，非阻塞）。
pub fn start(app_handle: AppHandleLite) -> Result<Bridge, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("AI 桥绑定失败：{e}"))?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let token = random_token();
    let stop = Arc::new(AtomicBool::new(false));
    let bridge = Bridge { port, token: token.clone(), stop: stop.clone() };
    let token2 = token.clone();
    std::thread::spawn(move || {
        let _ = listener.set_nonblocking(false);
        for stream in listener.incoming() {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            let Ok(stream) = stream else { continue };
            let token = token2.clone();
            let handle = app_handle.clone();
            std::thread::spawn(move || {
                let _ = handle_connection(stream, &token, &handle);
            });
        }
    });
    Ok(bridge)
}

/// 桥需要的运行时上下文（客户端 + 数据目录），避免依赖 tauri。
#[derive(Clone)]
pub struct AppHandleLite {
    pub client: reqwest::Client,
    pub data: std::path::PathBuf,
}

fn random_token() -> String {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(
        format!(
            "{}{}{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
            std::thread::current().name().unwrap_or("")
        )
        .as_bytes(),
    );
    hex::encode(hasher.finalize())
}

fn handle_connection(mut stream: TcpStream, token: &str, ctx: &AppHandleLite) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(60)))?;
    let mut buf = Vec::with_capacity(8192);
    let mut chunk = [0u8; 4096];
    let header_end;
    loop {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = find_subsequence(&buf, b"\r\n\r\n") {
            header_end = pos + 4;
            break;
        }
        if buf.len() > 64 * 1024 {
            return respond(&mut stream, 413, &json!({"error":"header too large"}));
        }
    }
    let header_text = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = header_text.lines();
    let request_line = lines.next().unwrap_or_default().to_string();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");

    // Bearer 校验
    let authed = lines.clone().any(|l| {
        let l = l.to_ascii_lowercase();
        l.starts_with("authorization: bearer ")
            && l.trim_start_matches("authorization: bearer ").trim() == token
    });
    if !authed {
        return respond(&mut stream, 401, &json!({"error":"unauthorized"}));
    }

    // Content-Length 补读 body
    let content_length = lines
        .find_map(|l| {
            let l = l.to_ascii_lowercase();
            l.strip_prefix("content-length:")
                .and_then(|v| v.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    let mut body = buf[header_end..].to_vec();
    while body.len() < content_length {
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    let parsed: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!({}));

    if method != "POST" {
        return respond(&mut stream, 405, &json!({"error":"POST only"}));
    }
    match path {
        "/hello" => {
            let greeting = format!(
                "我是小七。跟着你，需要就叫 #ai（跟随 / 停下 / 过来 / 看我，其余交给 AI）。"
            );
            respond(&mut stream, 200, &json!({"greeting": greeting}))
        }
        "/chat" => {
            let message = parsed.get("message").and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
            if message.is_empty() {
                return respond(&mut stream, 400, &json!({"error":"message required"}));
            }
            let reply = chat_reply(ctx, &message);
            respond(&mut stream, 200, &reply)
        }
        "/observe" => respond(&mut stream, 200, &json!({"remark": ""})),
        _ => respond(&mut stream, 404, &json!({"error":"not found"})),
    }
}

/// 聊天决策：规则优先 → LLM → 规则提示。
fn chat_reply(_ctx: &AppHandleLite, message: &str) -> Value {
    let lower = message.to_lowercase();
    // 1) 本地规则（与模组端保持一致，但这里是权威：模组可能没匹配到）
    for (keywords, action, reply) in RULES {
        if keywords.iter().any(|k| lower.contains(k)) {
            return json!({ "reply": reply, "action": action });
        }
    }
    // 2) LLM
    let cfg = crate::settings::load();
    if let (Some(base), Some(key)) = (cfg.ai_base_url.as_ref(), cfg.ai_key.as_ref()) {
        if !base.trim().is_empty() && !key.trim().is_empty() {
            if let Some(reply) = llm_chat(&cfg, base.trim(), key.trim(), message) {
                return json!({ "reply": reply, "action": "none" });
            }
            return json!({ "reply": "（AI 服务暂时没响应，我先用规则陪你：跟随 / 停下 / 过来 / 看我）", "action": "none" });
        }
    }
    // 3) 无 AI：规则提示
    json!({
        "reply": format!("我暂时只能听懂基础指令（跟随 / 停下 / 过来 / 看我）——「{message}」这类问题需要启动器里配置 AI Key。"),
        "action": "none"
    })
}

type Rule = (&'static [&'static str], &'static str, &'static str);
const RULES: &[Rule] = &[
    (&["跟随", "跟着", "跟上", "follow"], "follow", "好，跟着你。"),
    (&["停下", "待命", "别跟", "stop", "stay", "别动"], "stop", "好，我在这儿等你。"),
    (&["过来", "come", "来这里", "传送"], "come", "来啦。"),
    (&["看我", "look", "看着我", "面向我"], "look", "嗯，在看你。"),
    (&["配方", "recipe", "怎么做"], "none", "配方我可以提示：按 E 打开物品栏，点书图标就是配方书；模组配方大多在 JEI/REI 里查。"),
];

/// 调用 OpenAI 兼容 /chat/completions（阻塞，调用方在独立线程）。
fn llm_chat(cfg: &crate::settings::Config, base: &str, key: &str, message: &str) -> Option<String> {
    let url = format!("{}/chat/completions", base.trim_end_matches('/'));
    let model = cfg
        .ai_model
        .clone()
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| "deepseek-chat".to_string());
    let system = "你是 Minecraft 客户端里的 AI 伙伴「小七」：说话简短温和，1-3 句，\
                  可以给游戏建议，但绝不声称替玩家操作游戏；玩家可以用 #ai 跟随/停下/过来/看我 指挥你。";
    let body = json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": message}
        ],
        "max_tokens": 220,
        "temperature": 0.8
    });
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().ok()?;
    let resp = rt.block_on(async {
        http_client()?
            .post(&url)
            .bearer_auth(key)
            .json(&body)
            .timeout(Duration::from_secs(40))
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .json::<Value>()
            .await
            .ok()
    })?;
    let text = resp
        .pointer("/choices/0/message/content")
        .and_then(|x| x.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())?;
    Some(text)
}

fn http_client() -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .http1_only()
        .user_agent("mc-native-exp/ai-bridge")
        .timeout(Duration::from_secs(45))
        .build()
        .ok()
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn respond(stream: &mut TcpStream, status: u16, body: &Value) -> std::io::Result<()> {
    let payload = serde_json::to_string(body).unwrap_or_else(|_| "{}".into());
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        _ => "Error",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    stream.write_all(response.as_bytes())?;
    stream.flush()
}
