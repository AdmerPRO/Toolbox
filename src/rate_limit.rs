use anyhow::{Context, Result};
use axum::{
    extract::{ConnectInfo, Request, State},
    http::{Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const WINDOW: Duration = Duration::from_secs(60);
const MAX_CLIENTS: usize = 10_000;

#[derive(Clone, Copy)]
pub struct ClientIp(pub IpAddr);

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum Bucket {
    Api,
    Job,
}

struct Counter {
    started: Instant,
    count: u32,
}
struct Counters {
    entries: HashMap<(IpAddr, Bucket), Counter>,
    cleaned: Instant,
}

pub struct RateLimiter {
    counters: Mutex<Counters>,
    active_jobs: Mutex<HashMap<IpAddr, u32>>,
    concurrent_job_limit: u32,
    api_limit: u32,
    job_limit: u32,
    trusted_proxies: Vec<IpAddr>,
}

impl RateLimiter {
    pub fn from_env() -> Result<Arc<Self>> {
        fn limit(name: &str, default: u32) -> Result<u32> {
            let value = match std::env::var(name) {
                Ok(value) => value
                    .parse::<u32>()
                    .with_context(|| format!("{name} must be a positive integer"))?,
                Err(std::env::VarError::NotPresent) => default,
                Err(error) => return Err(error.into()),
            };
            anyhow::ensure!(
                (1..=100_000).contains(&value),
                "{name} must be between 1 and 100000"
            );
            Ok(value)
        }
        let trusted_proxies = std::env::var("TRUSTED_PROXY_IPS")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|ip| !ip.is_empty())
            .map(|ip| {
                ip.parse::<IpAddr>()
                    .map(|ip| ip.to_canonical())
                    .with_context(|| format!("Invalid trusted proxy IP: {ip}"))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Arc::new(Self {
            active_jobs: Mutex::new(HashMap::new()),
            concurrent_job_limit: limit("MAX_CONCURRENT_JOBS_PER_CLIENT", 1)?,
            counters: Mutex::new(Counters {
                entries: HashMap::new(),
                cleaned: Instant::now(),
            }),
            api_limit: limit("RATE_LIMIT_API_PER_MINUTE", 60)?,
            job_limit: limit("RATE_LIMIT_JOBS_PER_MINUTE", 10)?,
            trusted_proxies,
        }))
    }

    fn client_ip(&self, peer: IpAddr, forwarded: Option<&str>) -> IpAddr {
        let peer = peer.to_canonical();
        if !self.trusted_proxies.contains(&peer) {
            return peer;
        }
        let Some(header) = forwarded.filter(|value| value.len() <= 1024) else {
            return peer;
        };
        let addresses = header
            .split(',')
            .map(|value| value.trim().parse::<IpAddr>().map(|ip| ip.to_canonical()))
            .collect::<Result<Vec<_>, _>>();
        let Ok(addresses) = addresses else {
            return peer;
        };
        if addresses.len() > 16 {
            return peer;
        }
        // Walk from the nearest proxy toward the client, ignoring only explicitly trusted hops.
        addresses
            .into_iter()
            .rev()
            .find(|ip| !self.trusted_proxies.contains(ip))
            .unwrap_or(peer)
    }

    fn request_ip(&self, peer: IpAddr, headers: &axum::http::HeaderMap) -> IpAddr {
        let peer = peer.to_canonical();
        if self.trusted_proxies.contains(&peer) {
            let mut values = headers.get_all("cf-connecting-ip").iter();
            if let Some(value) = values.next() {
                // An invalid or repeated authoritative header fails closed to the proxy IP.
                return if values.next().is_none() {
                    value
                        .to_str()
                        .ok()
                        .and_then(|v| v.parse::<IpAddr>().ok())
                        .map(|ip| ip.to_canonical())
                        .unwrap_or(peer)
                } else {
                    peer
                };
            }
        }
        self.client_ip(
            peer,
            headers.get("x-forwarded-for").and_then(|h| h.to_str().ok()),
        )
    }

    fn acquire_job(self: &Arc<Self>, ip: IpAddr) -> Option<Arc<JobPermit>> {
        let ip = ip.to_canonical();
        let mut jobs = self
            .active_jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !jobs.contains_key(&ip) && jobs.len() >= MAX_CLIENTS {
            return None;
        }
        let count = jobs.entry(ip).or_default();
        if *count >= self.concurrent_job_limit {
            return None;
        }
        *count += 1;
        Some(Arc::new(JobPermit {
            limiter: self.clone(),
            ip,
        }))
    }

    fn check(&self, ip: IpAddr, bucket: Bucket, now: Instant) -> Result<(), u64> {
        let mut state = self
            .counters
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if now.duration_since(state.cleaned) >= WINDOW {
            state
                .entries
                .retain(|_, counter| now.duration_since(counter.started) < WINDOW);
            state.cleaned = now;
        }
        let key = (ip.to_canonical(), bucket);
        if !state.entries.contains_key(&key) && state.entries.len() >= MAX_CLIENTS * 2 {
            return Err(60);
        }
        let counter = state.entries.entry(key).or_insert(Counter {
            started: now,
            count: 0,
        });
        let elapsed = now.duration_since(counter.started);
        if elapsed >= WINDOW {
            counter.started = now;
            counter.count = 0;
        }
        let maximum = match bucket {
            Bucket::Api => self.api_limit,
            Bucket::Job => self.job_limit,
        };
        if counter.count >= maximum {
            let remaining = WINDOW.saturating_sub(now.duration_since(counter.started));
            return Err(remaining.as_secs() + u64::from(remaining.subsec_nanos() != 0));
        }
        counter.count += 1;
        Ok(())
    }
}

// Clones keep the same slot alive, including while a blocking image worker finishes.
pub struct JobPermit {
    limiter: Arc<RateLimiter>,
    ip: IpAddr,
}

impl Drop for JobPermit {
    fn drop(&mut self) {
        let mut jobs = self
            .limiter
            .active_jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(count) = jobs.get_mut(&self.ip) {
            *count -= 1;
            if *count == 0 {
                jobs.remove(&self.ip);
            }
        }
    }
}

pub async fn middleware(
    State(limiter): State<Arc<RateLimiter>>,
    mut request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path().to_owned();
    if !path.starts_with("/api/") || path == "/api/healthcheck" {
        return next.run(request).await;
    }
    let Some(ConnectInfo(peer)) = request.extensions().get::<ConnectInfo<SocketAddr>>() else {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Client address is unavailable.",
        )
            .into_response();
    };
    let ip = limiter.request_ip(peer.ip(), request.headers());
    request.extensions_mut().insert(ClientIp(ip));
    let bucket = if request.method() == Method::POST {
        Bucket::Job
    } else {
        Bucket::Api
    };
    if let Err(seconds) = limiter.check(ip, bucket, Instant::now()) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [
                (header::RETRY_AFTER, seconds.max(1).to_string()),
                (header::CACHE_CONTROL, "no-store".into()),
            ],
            "Too many requests. Please wait before trying again.",
        )
            .into_response();
    }
    if bucket == Bucket::Job
        && matches!(
            path.as_str(),
            "/api/youtube/info" | "/api/youtube/download" | "/api/youtube/download/mp3"
        )
        || (bucket == Bucket::Job && path.starts_with("/api/convert/"))
    {
        let accepted = request
            .headers()
            .get(header::COOKIE)
            .and_then(|h| h.to_str().ok())
            .is_some_and(|value| {
                value.split(';').any(|cookie| {
                    cookie.trim() == format!("privacy_policy={}", crate::audit::POLICY_VERSION)
                })
            });
        if !accepted {
            return (
                StatusCode::PRECONDITION_REQUIRED,
                "Please read and accept the privacy policy before using media tools.",
            )
                .into_response();
        }
    }
    let client_permit = if bucket == Bucket::Job {
        let Some(permit) = limiter.acquire_job(ip) else {
            return (StatusCode::TOO_MANY_REQUESTS,
                [(header::RETRY_AFTER, "1"), (header::CACHE_CONTROL, "no-store")],
                "You already have too many operations running. Wait for one to finish before sending another.").into_response();
        };
        request.extensions_mut().insert(permit.clone());
        Some(permit)
    } else {
        None
    };
    let response = next.run(request).await;
    drop(client_permit);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limiter() -> RateLimiter {
        RateLimiter {
            active_jobs: Mutex::new(HashMap::new()),
            concurrent_job_limit: 1,
            counters: Mutex::new(Counters {
                entries: HashMap::new(),
                cleaned: Instant::now(),
            }),
            api_limit: 3,
            job_limit: 2,
            trusted_proxies: vec!["127.0.0.1".parse().unwrap()],
        }
    }

    #[test]
    fn limits_clients_and_buckets_independently_and_resets() {
        let limiter = limiter();
        let now = Instant::now();
        let ip = "192.0.2.1".parse().unwrap();
        for _ in 0..2 {
            assert!(limiter.check(ip, Bucket::Job, now).is_ok());
        }
        assert_eq!(
            limiter.check(ip, Bucket::Job, now + Duration::from_millis(100)),
            Err(60)
        );
        assert!(limiter.check(ip, Bucket::Api, now).is_ok());
        assert!(
            limiter
                .check("192.0.2.2".parse().unwrap(), Bucket::Job, now)
                .is_ok()
        );
        assert!(limiter.check(ip, Bucket::Job, now + WINDOW).is_ok());
    }

    #[test]
    fn trusts_forwarded_addresses_only_from_configured_proxies() {
        let limiter = limiter();
        let proxy = "127.0.0.1".parse().unwrap();
        let client = "192.0.2.1".parse().unwrap();
        assert_eq!(limiter.client_ip(client, Some("198.51.100.1")), client);
        assert_eq!(
            limiter.client_ip(proxy, Some("198.51.100.1, 192.0.2.1, 127.0.0.1")),
            client
        );
        assert_eq!(limiter.client_ip(proxy, Some("invalid, 192.0.2.1")), proxy);
        assert_eq!(limiter.client_ip(proxy, None), proxy);
    }

    #[test]
    fn cloudflare_headers_require_a_trusted_peer_and_one_valid_ip() {
        use axum::http::{HeaderMap, HeaderValue};
        let limiter = limiter();
        let proxy = "127.0.0.1".parse().unwrap();
        let client = "192.0.2.1".parse().unwrap();
        let mut headers = HeaderMap::new();
        headers.insert("cf-connecting-ip", HeaderValue::from_static("192.0.2.1"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("198.51.100.1"));
        assert_eq!(limiter.request_ip(proxy, &headers), client);
        assert_eq!(limiter.request_ip(client, &headers), client);
        headers.insert("cf-connecting-ip", HeaderValue::from_static("invalid"));
        assert_eq!(limiter.request_ip(proxy, &headers), proxy);
        headers.insert(
            "cf-connecting-ip",
            HeaderValue::from_static("::ffff:192.0.2.1"),
        );
        assert_eq!(limiter.request_ip(proxy, &headers), client);
        headers.append("cf-connecting-ip", HeaderValue::from_static("198.51.100.1"));
        assert_eq!(limiter.request_ip(proxy, &headers), proxy);
    }

    #[test]
    fn concurrent_requests_cannot_exceed_limit() {
        let limiter = Arc::new(limiter());
        let now = Instant::now();
        let handles = (0..20)
            .map(|_| {
                let limiter = limiter.clone();
                std::thread::spawn(move || {
                    limiter
                        .check("192.0.2.1".parse().unwrap(), Bucket::Job, now)
                        .is_ok()
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(
            handles
                .into_iter()
                .filter_map(|handle| handle.join().ok())
                .filter(|allowed| *allowed)
                .count(),
            2
        );
    }

    #[test]
    fn one_client_cannot_occupy_other_clients_job_slots() {
        let limiter = Arc::new(limiter());
        let ip = "192.0.2.1".parse().unwrap();
        let first = limiter.acquire_job(ip).unwrap();
        assert!(limiter.acquire_job(ip).is_none());
        let other = limiter.acquire_job("192.0.2.2".parse().unwrap()).unwrap();
        let worker = first.clone();
        drop(first);
        assert!(limiter.acquire_job(ip).is_none());
        drop(worker);
        let retry = limiter.acquire_job(ip).unwrap();
        drop(retry);
        drop(other);
        assert!(limiter.active_jobs.lock().unwrap().is_empty());
    }
}
