//! 微软正版账号认证：MSA 设备码流程 → Xbox Live → XSTS → Minecraft services
//! token → 所有权（entitlements）校验 → profile。无正版账号实验拒绝继续。
//!
//! 端点链与 Aurora Launcher（src-tauri/src/auth.rs）一致，均为公开文档化的
//! OAuth / Xbox / Minecraft services 接口；token 只落本地 data\token.json。

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::manifest;

const DEVICECODE_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
const TOKEN_URL: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const SCOPE: &str = "XboxLive.signin offline_access";
/// 用户自己的 Azure 应用注册 id；env MC_EXP_CLIENT_ID 可覆盖。
const FALLBACK_CLIENT_ID: &str = "ae897ba3-93b3-4998-a4fb-b80bad87b538";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Session {
    /// Minecraft services access_token（jwt）。
    pub mc_access: String,
    pub mc_expires_at: u64,
    /// 刷新用的 MSA refresh_token。
    pub refresh_token: String,
    pub player_name: String,
    pub player_uuid: String,
    pub client_id: String,
}

#[derive(Debug, Deserialize)]
struct DeviceCode {
    device_code: String,
    user_code: String,
    verification_uri: String,
    #[serde(default)]
    direct_verification_uri: Option<String>,
    expires_in: u64,
    interval: u64,
}

fn client_id() -> String {
    std::env::var("MC_EXP_CLIENT_ID")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| FALLBACK_CLIENT_ID.to_string())
}

fn token_path(data: &Path) -> PathBuf {
    data.join("token.json")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn load_cached(data: &Path) -> Option<Session> {
    let text = std::fs::read_to_string(token_path(data)).ok()?;
    let s: Session = serde_json::from_str(&text).ok()?;
    (s.mc_expires_at > now() + 60).then_some(s)
}

/// 有效会话：缓存未过期直接用；否则用 refresh_token 静默续期；再不行走设备码。
pub async fn ensure_session(client: &reqwest::Client, data: &Path) -> Result<Session, String> {
    if let Some(s) = load_cached(data) {
        println!("已用缓存的正版会话：{} ({})", s.player_name, s.player_uuid);
        return Ok(s);
    }
    if let Ok(text) = std::fs::read_to_string(token_path(data)) {
        if let Ok(old) = serde_json::from_str::<Session>(&text) {
            if let Ok(s) = refresh(client, &old).await {
                save(data, &s)?;
                println!("token 已静默续期：{}", s.player_name);
                return Ok(s);
            }
        }
    }
    login(client, data).await
}

/// 设备码登录：打印码 → 打开浏览器 → 轮询 → 全链校验。
pub async fn login(client: &reqwest::Client, data: &Path) -> Result<Session, String> {
    let cid = client_id();
    let dc: DeviceCode = client
        .post(DEVICECODE_URL)
        .form(&[("client_id", cid.as_str()), ("scope", SCOPE)])
        .send()
        .await
        .and_then(|r| r.error_for_status())
        .map_err(|e| format!("请求设备码失败：{e}（检查 MC_EXP_CLIENT_ID）"))?
        .json()
        .await
        .map_err(|e| format!("解析设备码失败：{e}"))?;

    println!("微软正版账号验证（实验前提：必须拥有 Minecraft: Java Edition）");
    println!("  打开 {}", dc.verification_uri);
    println!("  输入代码 {}", dc.user_code);
    if let Some(direct) = &dc.direct_verification_uri {
        println!("  （或直接打开 {}）", direct);
    }
    let _ = open::that(dc.direct_verification_uri.as_deref().unwrap_or(&dc.verification_uri));

    let interval = Duration::from_secs(dc.interval.max(3).min(15));
    let deadline = std::time::Instant::now() + Duration::from_secs(dc.expires_in);
    let ms_token = loop {
        if std::time::Instant::now() > deadline {
            return Err("设备码已过期，重新运行 auth".into());
        }
        tokio::time::sleep(interval).await;
        let resp = client
            .post(TOKEN_URL)
            .form(&[
                ("client_id", cid.as_str()),
                ("device_code", dc.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await
            .map_err(|e| format!("轮询 token 失败：{e}"))?;
        let v: serde_json::Value = resp.json().await.map_err(|e| format!("{e}"))?;
        if let Some(tok) = v.get("access_token").and_then(|x| x.as_str()) {
            let refresh = v
                .get("refresh_token")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            break (tok.to_string(), refresh);
        }
        match v.get("error").and_then(|x| x.as_str()) {
            Some("authorization_pending") => continue,
            Some("slow_down") => tokio::time::sleep(Duration::from_secs(5)).await,
            Some("invalid_grant") | Some("expired_token") => {
                println!();
                println!("⚠ 设备码已过期——自动签发新码，请用新码完成登录：");
                return Box::pin(login(client, data)).await;
            }
            Some(other) => {
                let desc = v
                    .get("error_description")
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                return Err(format!("登录被拒绝：{other} {desc}"));
            }
            None => return Err("token 响应格式异常".into()),
        }
    };

    let session = finish_login(client, &ms_token.0, &ms_token.1, &cid).await?;
    save(data, &session)?;
    println!(
        "正版账号验证通过：{} ({})——Minecraft: Java Edition 所有权确认",
        session.player_name, session.player_uuid
    );
    Ok(session)
}

/// MSA token → XBL → XSTS → MC token → entitlements → profile。
async fn finish_login(
    client: &reqwest::Client,
    ms_access: &str,
    refresh_token: &str,
    cid: &str,
) -> Result<Session, String> {
    // 1) Xbox Live user token
    let xbox: serde_json::Value = client
        .post("https://user.auth.xboxlive.com/user/authenticate")
        .json(&serde_json::json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("d={ms_access}"),
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT",
        }))
        .send()
        .await
        .map_err(|e| format!("Xbox Live 认证失败：{e}"))?
        .error_for_status()
        .map_err(|e| format!("Xbox Live 认证被拒绝：{e}"))?
        .json()
        .await
        .map_err(|e| format!("Xbox 响应解析失败：{e}"))?;
    let xbl_token = xbox
        .get("Token")
        .and_then(|x| x.as_str())
        .ok_or("Xbox 响应缺少 Token")?;
    let uhs = xbox
        .pointer("/DisplayClaims/xui/0/uhs")
        .and_then(|x| x.as_str())
        .ok_or("Xbox 响应缺少 uhs")?
        .to_string();

    // 2) XSTS（接口偶发非 JSON 响应：重试 + 宽松解析，最终失败自动换码）
    let xsts: serde_json::Value = {
        let mut got = None;
        for attempt in 0..3u32 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(10 * attempt as u64)).await;
            }
            let resp = client
                .post("https://xsts.auth.xboxlive.com/xsts/authorize")
                .json(&serde_json::json!({
                    "Properties": {
                        "SandboxId": "RETAIL",
                        "UserTokens": [xbl_token],
                    },
                    "RelyingParty": "rp://api.minecraftservices.com/",
                    "TokenType": "JWT",
                }))
                .send()
                .await
                .map_err(|e| format!("XSTS 认证失败：{e}"))?;
            let status = resp.status();
            let text = resp.text().await.map_err(|e| format!("XSTS 读取失败：{e}"))?;
            println!("XSTS HTTP {}（{} B）", status.as_u16(), text.len());
            match serde_json::from_str(&text) {
                Ok(v) => {
                    got = Some(v);
                    break;
                }
                Err(_) => {
                    println!("XSTS 第 {} 次响应非 JSON（{} B），重试……", attempt + 1, text.len());
                }
            }
        }
        got.ok_or("XSTS 连续返回异常响应——请稍后重试或换网络环境")?
    };
    let xsts_token = xsts
        .get("Token")
        .and_then(|x| x.as_str())
        .ok_or_else(|| {
            let code = xsts.get("XErr").and_then(|x| x.as_u64()).unwrap_or(0);
            match code {
                2148916233 => "该微软账号没有 Xbox 档案，请先在 xbox.com 创建".to_string(),
                2148916238 => "该账号是儿童账户，需家长在家庭设置里允许".to_string(),
                _ => format!("XSTS 认证失败（XErr {code}）"),
            }
        })?;

    // 3) Minecraft services token（同样宽松解析 + 重试）
    let mc: serde_json::Value = {
        let mut got = None;
        for attempt in 0..3u32 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_secs(3 * attempt as u64)).await;
            }
            let resp = client
                .post("https://api.minecraftservices.com/authentication/login_with_xbox")
                .json(&serde_json::json!({
                    "identityToken": format!("XBL3.0 x={uhs};{xsts_token}"),
                }))
                .send()
                .await
                .map_err(|e| format!("Minecraft 登录失败：{e}"))?;
            let status = resp.status();
            let text = resp.text().await.map_err(|e| format!("MC 读取失败：{e}"))?;
            match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(v) if status.is_success() => {
                    got = Some(v);
                    break;
                }
                _ => {
                    println!("MC token 第 {} 次异常（HTTP {}，{} B），重试……", attempt + 1, status.as_u16(), text.len());
                }
            }
        }
        got.ok_or("MC token 连续异常——请稍后重试")?
    };
    let mc_access = mc
        .get("access_token")
        .and_then(|x| x.as_str())
        .ok_or("MC 响应缺少 access_token")?
        .to_string();
    let expires_in = mc.get("expires_in").and_then(|x| x.as_u64()).unwrap_or(86400);

    // 4) 所有权校验：entitlements 不能为空（正版前提）
    let ent: serde_json::Value = client
        .get("https://api.minecraftservices.com/entitlements/mcstore")
        .bearer_auth(&mc_access)
        .send()
        .await
        .map_err(|e| format!("检查所有权失败：{e}"))?
        .error_for_status()
        .map_err(|e| format!("检查所有权失败：{e}"))?
        .json()
        .await
        .map_err(|e| format!("entitlements 解析失败：{e}"))?;
    let items = ent
        .get("items")
        .and_then(|x| x.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    if items == 0 {
        return Err("该账户没有 Minecraft: Java Edition 所有权——本实验要求正版账号，拒绝继续".into());
    }

    // 5) profile
    let profile: serde_json::Value = client
        .get("https://api.minecraftservices.com/minecraft/profile")
        .bearer_auth(&mc_access)
        .send()
        .await
        .map_err(|e| format!("获取档案失败：{e}"))?
        .error_for_status()
        .map_err(|e| format!("获取档案失败：{e}（该账号可能没有创建 Java 档案）"))?
        .json()
        .await
        .map_err(|e| format!("profile 解析失败：{e}"))?;
    let name = profile
        .get("name")
        .and_then(|x| x.as_str())
        .ok_or("profile 缺少 name")?
        .to_string();
    let uuid = profile
        .get("id")
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string();

    Ok(Session {
        mc_access,
        mc_expires_at: now() + expires_in.saturating_sub(60),
        refresh_token: refresh_token.to_string(),
        player_name: name,
        player_uuid: uuid,
        client_id: cid.to_string(),
    })
}

/// refresh_token 静默续期，然后重走全链。
async fn refresh(client: &reqwest::Client, old: &Session) -> Result<Session, String> {
    let cid = client_id();
    let v: serde_json::Value = client
        .post(TOKEN_URL)
        .form(&[
            ("client_id", cid.as_str()),
            ("refresh_token", old.refresh_token.as_str()),
            ("grant_type", "refresh_token"),
            ("scope", SCOPE),
        ])
        .send()
        .await
        .map_err(|e| format!("{e}"))?
        .json()
        .await
        .map_err(|e| format!("{e}"))?;
    let ms_access = v
        .get("access_token")
        .and_then(|x| x.as_str())
        .ok_or("refresh 被拒，需要重新设备码登录")?;
    let refresh_token = v
        .get("refresh_token")
        .and_then(|x| x.as_str())
        .unwrap_or(&old.refresh_token)
        .to_string();
    finish_login(client, ms_access, &refresh_token, &cid).await
}

fn save(data: &Path, s: &Session) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?;
    manifest::write_atomic(&token_path(data), &bytes)
}

/// 供 fetch/interp 阶段使用：确保有正版会话并返回。
#[allow(dead_code)]
pub async fn require(client: &reqwest::Client, data: &Path) -> Result<Session, String> {
    ensure_session(client, data).await
}
