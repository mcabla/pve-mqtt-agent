//! Proxmox API client (token auth, pool discovery, power actions, status polling).
//!
//! Notes on stability:
//! - Pool read endpoint may move/deprecate; we try `/pools/{pool}` first, then fallback
//!   to `/pools?poolid={pool}` for compatibility with alternate API shapes.

use std::{collections::HashMap, num::NonZeroU32, time::Duration};

use reqwest::{header, Client};
use serde::Deserialize;
use thiserror::Error;

use crate::topic::{Action, GuestType};

#[derive(Debug, Error)]
pub enum ProxmoxError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("unexpected response: {0}")]
    Unexpected(String),
    #[error("resource not found")]
    NotFound,
    #[error("permission denied")]
    Forbidden,
}

/// Minimal config required to talk to Proxmox.
#[derive(Debug, Clone)]
pub struct ProxmoxConfig {
    pub base_url: String,
    pub token_id: String,
    pub token_secret: String,
    pub pool: String,
    pub timeout: Duration,
    pub ca_pem: Option<Vec<u8>>,
}

/// Resolved guest info (from pool membership).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestRef {
    pub guest_type: GuestType,
    pub vmid: NonZeroU32,
    pub node: String,
    pub name: Option<String>,
}

/// Guest status (subset).
#[derive(Debug, Clone)]
pub struct GuestStatus {
    pub status: String,
    pub cpu: Option<f64>,
    pub mem: Option<u64>,
    pub maxmem: Option<u64>,
    pub disk: Option<u64>,
    pub maxdisk: Option<u64>,
    pub uptime: Option<u64>,
}

#[derive(Clone)]
pub struct ProxmoxClient {
    cfg: ProxmoxConfig,
    http: Client,
}

impl ProxmoxClient {
    pub fn new(cfg: ProxmoxConfig) -> Result<Self, ProxmoxError> {
        let mut headers = header::HeaderMap::new();
        // Format: PVEAPIToken=USER@REALM!TOKENID=SECRET
        let token = format!("PVEAPIToken={}={}", cfg.token_id, cfg.token_secret);
        headers.insert(header::AUTHORIZATION, header::HeaderValue::from_str(&token)
            .map_err(|_| ProxmoxError::Unexpected("invalid token header".into()))?);
        headers.insert(header::ACCEPT, header::HeaderValue::from_static("application/json"));

        let mut builder = Client::builder()
            .default_headers(headers)
            .timeout(cfg.timeout);

        if cfg.base_url.starts_with("https://") {
            if let Some(pem) = &cfg.ca_pem {
                let cert = reqwest::Certificate::from_pem(pem)
                    .map_err(|e| ProxmoxError::Unexpected(format!("invalid proxmox CA pem: {e}")))?;
                builder = builder.tls_certs_merge(std::iter::once(cert));
            } else {
                // Keep backward-compatible behavior for self-signed lab setups.
                builder = builder.danger_accept_invalid_certs(true);
            }
        }

        let http = builder.build()?;
        Ok(Self { cfg, http })
    }

    fn url(&self, path: &str) -> String {
        format!("{}/api2/json{}", self.cfg.base_url.trim_end_matches('/'), path)
    }

    /// Fetch pool members (allowed guests).
    pub async fn pool_members(&self) -> Result<Vec<GuestRef>, ProxmoxError> {
        // Try modern/known endpoint first: GET /pools/{poolid}
        match self.pool_members_via_path().await {
            Ok(v) => Ok(v),
            Err(ProxmoxError::NotFound) => self.pool_members_via_query().await,
            Err(e) => {
                // Fallback on some other failures that may indicate endpoint changes.
                self.pool_members_via_query().await.or(Err(e))
            }
        }
    }

    async fn pool_members_via_path(&self) -> Result<Vec<GuestRef>, ProxmoxError> {
        let path = format!("/pools/{}", urlencoding::encode(&self.cfg.pool));
        let resp = self.http.get(self.url(&path)).send().await?;
        Self::map_status(&resp)?;
        let body: ApiEnvelope<PoolRead> = resp.json().await?;
        Ok(parse_pool_members(body.data.members.unwrap_or_default()))
    }

    async fn pool_members_via_query(&self) -> Result<Vec<GuestRef>, ProxmoxError> {
        // Compatibility fallback: /pools?poolid=...
        let path = format!("/pools?poolid={}", urlencoding::encode(&self.cfg.pool));
        let resp = self.http.get(self.url(&path)).send().await?;
        Self::map_status(&resp)?;
        let body: ApiEnvelope<Vec<PoolRead>> = resp.json().await?;
        let pool = body.data.into_iter().next()
            .ok_or_else(|| ProxmoxError::Unexpected("pool query returned empty list".into()))?;
        Ok(parse_pool_members(pool.members.unwrap_or_default()))
    }

    /// Get current status for a guest.
    pub async fn guest_status(&self, g: &GuestRef) -> Result<GuestStatus, ProxmoxError> {
        let path = format!(
            "/nodes/{}/{}/{}/status/current",
            urlencoding::encode(&g.node),
            g.guest_type.api_segment(),
            g.vmid.get()
        );
        let resp = self.http.get(self.url(&path)).send().await?;
        Self::map_status(&resp)?;
        let body: ApiEnvelope<StatusCurrent> = resp.json().await?;
        Ok(GuestStatus {
            status: body.data.status.unwrap_or_else(|| "unknown".into()),
            cpu: body.data.cpu,
            mem: body.data.mem,
            maxmem: body.data.maxmem,
            disk: body.data.disk,
            maxdisk: body.data.maxdisk,
            uptime: body.data.uptime,
        })
    }

    /// Execute a power action on a guest.
    pub async fn power_action(&self, g: &GuestRef, action: Action) -> Result<String, ProxmoxError> {
        let path = format!(
            "/nodes/{}/{}/{}/status/{}",
            urlencoding::encode(&g.node),
            g.guest_type.api_segment(),
            g.vmid.get(),
            action.api_segment()
        );
        let resp = self.http.post(self.url(&path)).send().await?;
        Self::map_status(&resp)?;
        let body: ApiEnvelope<ActionResult> = resp.json().await?;
        // Most power operations return a task UPID in `data`.
        Ok(body.data.data.unwrap_or_default())
    }

    fn map_status(resp: &reqwest::Response) -> Result<(), ProxmoxError> {
        match resp.status().as_u16() {
            200..=299 => Ok(()),
            401 | 403 => Err(ProxmoxError::Forbidden),
            404 => Err(ProxmoxError::NotFound),
            s => Err(ProxmoxError::Unexpected(format!("http status {s}"))),
        }
    }
}

#[derive(Debug, Deserialize)]
struct ApiEnvelope<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
struct PoolRead {
    #[serde(default)]
    members: Option<Vec<PoolMember>>,
}

#[derive(Debug, Deserialize)]
struct PoolMember {
    /// Example: "qemu/100" or "lxc/201"
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    node: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StatusCurrent {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    cpu: Option<f64>,
    #[serde(default)]
    mem: Option<u64>,
    #[serde(default)]
    maxmem: Option<u64>,
    #[serde(default)]
    disk: Option<u64>,
    #[serde(default)]
    maxdisk: Option<u64>,
    #[serde(default)]
    uptime: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ActionResult {
    #[serde(default)]
    data: Option<String>,
}

fn parse_pool_members(members: Vec<PoolMember>) -> Vec<GuestRef> {
    let mut out = Vec::with_capacity(members.len());
    for m in members {
        let (Some(id), Some(node)) = (m.id, m.node) else { continue };
        let mut parts = id.split('/');
        let Some(kind) = parts.next() else { continue };
        let Some(vmid_s) = parts.next() else { continue };
        if parts.next().is_some() { continue }

        let guest_type = match kind {
            "qemu" => GuestType::Qemu,
            "lxc" => GuestType::Lxc,
            _ => continue,
        };
        let vmid: u32 = match vmid_s.parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let Some(vmid) = NonZeroU32::new(vmid) else { continue };

        out.push(GuestRef { guest_type, vmid, node, name: m.name });
    }
    out
}

/// Build a quick lookup map from (type, vmid) to GuestRef.
pub fn index_members(members: Vec<GuestRef>) -> HashMap<(GuestType, NonZeroU32), GuestRef> {
    members
        .into_iter()
        .map(|g| ((g.guest_type, g.vmid), g))
        .collect()
}
