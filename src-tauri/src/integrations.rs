//! Optional integrations. Each one is off until the user turns it on in Settings and gives its address;
//! tokens and passwords come from the Keychain (secrets.rs). Every call is read-only except the few
//! explicit actions the user clicks: switch lights, start a stopped VM/container, restart a Docker
//! container, pause Pi-hole blocking.
//!
//! Lights: only Home Assistant `light.*` entities are ever switched, never switches or plugs.

use crate::{secrets, AppState};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Integ {
    pub on: bool,
    pub url: String,
    /// Proxmox token id, qBittorrent user name, stock alert %, weather units, Portainer endpoint …
    pub user: String,
    /// Stock symbols, weather place, Lian Li seconds …
    pub extra: String,
    /// Accept a self-signed HTTPS certificate (Proxmox, Portainer on the home network).
    pub insecure: bool,
}

pub const NAMES: &[&str] = &["home_assistant", "proxmox", "portainer", "pihole", "jellyfin", "radarr", "sonarr", "qbittorrent", "stocks", "weather", "lianli"];

pub fn label(name: &str) -> &'static str {
    match name {
        "home_assistant" => "Home Assistant",
        "proxmox" => "Proxmox",
        "portainer" => "Portainer",
        "pihole" => "Pi-hole",
        "jellyfin" => "Jellyfin",
        "radarr" => "Radarr",
        "sonarr" => "Sonarr",
        "qbittorrent" => "qBittorrent",
        "stocks" => "Stocks",
        "weather" => "Weather",
        "lianli" => "Lian Li screen",
        _ => "That integration",
    }
}

pub fn err<E: std::fmt::Display>(e: E) -> String {
    let s = e.to_string();
    if s.contains("timed out") {
        "it didn't answer in time".into()
    } else if s.contains("certificate") || s.contains("Certificate") {
        "its HTTPS certificate isn't trusted (tick “Self-signed certificate” in Settings)".into()
    } else if s.contains("connect") || s.contains("dns error") {
        "it can't be reached on the network".into()
    } else {
        s
    }
}

async fn read(r: reqwest::Response) -> Result<Value, String> {
    let code = r.status();
    let text = r.text().await.map_err(err)?;
    let v: Value = serde_json::from_str(&text).unwrap_or_else(|_| json!({"text": text.chars().take(300).collect::<String>()}));
    if code.as_u16() == 401 || code.as_u16() == 403 {
        return Err(format!("it refused the login (HTTP {}). Check the token or password in Settings.", code.as_u16()));
    }
    if !code.is_success() {
        let m = v.get("error").or_else(|| v.get("message")).and_then(|e| e.as_str()).map(String::from);
        return Err(m.unwrap_or_else(|| format!("HTTP {}", code.as_u16())));
    }
    Ok(v)
}

fn ymd(days_from_today: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let z = now / 86400 + days_from_today + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}

fn need(name: &str) -> Result<String, String> {
    secrets::get(name).ok_or_else(|| format!("{} needs its token in Settings", label(name)))
}

/// Runs one integration action. Read-only actions are tried once more when the connection dropped
/// (a server closing a reused connection, or several reads at once), never actions that change things.
pub async fn call(st: &AppState, name: &str, action: &str, args: &Value) -> Result<Value, String> {
    let r = call_once(st, name, action, args).await;
    let read_only = matches!(action, "status" | "test" | "feed" | "lights" | "states");
    match r {
        Err(e) if read_only && (e.contains("sending request") || e.contains("connection") || e.contains("closed")) => {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            call_once(st, name, action, args).await
        }
        other => other,
    }
}

async fn call_once(st: &AppState, name: &str, action: &str, args: &Value) -> Result<Value, String> {
    let cfg = st.settings.lock().integrations.get(name).cloned().unwrap_or_default();
    if !cfg.on && action != "test" {
        return Err(format!("{} is off in Settings", label(name)));
    }
    let base = cfg.url.trim().trim_end_matches('/').to_string();
    let web = !matches!(name, "stocks" | "weather");
    if web && !(base.starts_with("http://") || base.starts_with("https://")) {
        return Err(format!("{} needs its address in Settings (for example http://192.168.1.10:8123)", label(name)));
    }
    let http = if cfg.insecure { &st.http_insecure } else { &st.http };
    let t = |s: u64| Duration::from_secs(s);
    match (name, action) {
        // ---------- Home Assistant (lights only) ----------
        ("home_assistant", "test") => {
            let tok = need(name)?;
            let v = read(http.get(format!("{base}/api/")).bearer_auth(tok).timeout(t(8)).send().await.map_err(err)?).await?;
            Ok(json!({"message": v["message"].as_str().unwrap_or("Home Assistant answered")}))
        }
        ("home_assistant", "lights") => {
            let tok = need(name)?;
            let v = read(http.get(format!("{base}/api/states")).bearer_auth(tok).timeout(t(10)).send().await.map_err(err)?).await?;
            let lights: Vec<Value> = v.as_array().cloned().unwrap_or_default().into_iter()
                .filter(|e| e["entity_id"].as_str().map(|s| s.starts_with("light.")).unwrap_or(false))
                .map(|e| json!({"entity": e["entity_id"], "name": e["attributes"]["friendly_name"].as_str().unwrap_or(e["entity_id"].as_str().unwrap_or("light")),
                    "on": e["state"] == "on", "available": e["state"] != "unavailable"}))
                .collect();
            Ok(json!({"lights": lights}))
        }
        // Everything the chat may read: lights, climate, weather, useful sensors, doors and windows,
        // and switches/fans (read-only: only lights can ever be switched).
        ("home_assistant", "states") => {
            let tok = need(name)?;
            let v = read(http.get(format!("{base}/api/states")).bearer_auth(tok).timeout(t(6)).send().await.map_err(err)?).await?;
            const KINDS: [&str; 11] = ["temperature", "humidity", "pressure", "carbon_dioxide", "pm25", "pm10", "volatile_organic_compounds", "illuminance", "power", "energy", "battery"];
            const OPENINGS: [&str; 5] = ["door", "window", "opening", "garage_door", "lock"];
            let (mut lights, mut climate, mut weather, mut sensors, mut openings, mut switches) = (vec![], vec![], vec![], vec![], vec![], vec![]);
            for e in v.as_array().cloned().unwrap_or_default() {
                let id = e["entity_id"].as_str().unwrap_or("").to_string();
                let st = e["state"].as_str().unwrap_or("").to_string();
                let a = &e["attributes"];
                let nm = a["friendly_name"].as_str().unwrap_or(&id).to_string();
                let dc = a["device_class"].as_str().unwrap_or("");
                let (dom, _) = id.split_once('.').unwrap_or(("", ""));
                if st == "unavailable" || st == "unknown" {
                    if dom == "light" {
                        lights.push(json!({"entity": id, "name": nm, "on": false, "available": false}));
                    }
                    continue;
                }
                match dom {
                    "light" => lights.push(json!({"entity": id, "name": nm, "on": st == "on", "available": true,
                        "brightness": a["brightness"].as_f64().map(|b| (b / 2.55).round())})),
                    "climate" => climate.push(json!({"name": nm, "mode": st, "action": a["hvac_action"], "current": a["current_temperature"],
                        "target": a["temperature"], "humidity": a["current_humidity"]})),
                    "weather" => weather.push(json!({"name": nm, "state": st, "temperature": a["temperature"], "unit": a["temperature_unit"], "humidity": a["humidity"]})),
                    "sensor" if KINDS.contains(&dc) && sensors.len() < 80 => sensors.push(json!({"name": nm, "kind": dc, "value": st, "unit": a["unit_of_measurement"]})),
                    "binary_sensor" if OPENINGS.contains(&dc) && openings.len() < 40 => openings.push(json!({"name": nm, "open": st == "on"})),
                    "lock" if openings.len() < 40 => openings.push(json!({"name": nm, "locked": st == "locked"})),
                    "switch" | "fan" if switches.len() < 40 => switches.push(json!({"name": nm, "on": st == "on"})),
                    _ => {}
                }
            }
            Ok(json!({"lights": lights, "climate": climate, "weather": weather, "sensors": sensors, "openings": openings, "switches": switches}))
        }
        ("home_assistant", "set") => {
            let tok = need(name)?;
            let on = args["on"].as_bool().ok_or("say on or off")?;
            let ids: Vec<String> = match args["entities"].as_array() {
                Some(a) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
                None => vec![],
            };
            if ids.is_empty() || ids.iter().any(|e| !e.starts_with("light.") || e.len() > 120) {
                return Err("only lights can be switched".into());
            }
            let svc = if on { "turn_on" } else { "turn_off" };
            read(http.post(format!("{base}/api/services/light/{svc}")).bearer_auth(tok).json(&json!({"entity_id": ids})).timeout(t(15)).send().await.map_err(err)?).await?;
            Ok(json!({"ok": true, "on": on, "count": ids.len()}))
        }

        // ---------- Proxmox VE ----------
        ("proxmox", "status") | ("proxmox", "test") => {
            let auth = format!("PVEAPIToken={}={}", cfg.user.trim(), need(name)?);
            let v = read(http.get(format!("{base}/api2/json/cluster/resources")).header("Authorization", auth).timeout(t(10)).send().await.map_err(err)?).await?;
            let all = v["data"].as_array().cloned().unwrap_or_default();
            let nodes: Vec<Value> = all.iter().filter(|r| r["type"] == "node").map(|r| json!({
                "name": r["node"], "online": r["status"] == "online",
                "cpu": r["cpu"].as_f64().unwrap_or(0.0) * 100.0,
                "mem": r["mem"].as_f64().unwrap_or(0.0) / r["maxmem"].as_f64().unwrap_or(1.0).max(1.0) * 100.0,
            })).collect();
            let guests: Vec<Value> = all.iter().filter(|r| r["type"] == "qemu" || r["type"] == "lxc").map(|r| json!({
                "vmid": r["vmid"], "name": r["name"].as_str().unwrap_or(""), "type": r["type"], "node": r["node"],
                "running": r["status"] == "running", "template": r["template"].as_u64() == Some(1),
            })).filter(|g| g["template"] != true).collect();
            Ok(json!({"nodes": nodes, "guests": guests}))
        }
        ("proxmox", "start") => {
            let auth = format!("PVEAPIToken={}={}", cfg.user.trim(), need(name)?);
            let node = args["node"].as_str().unwrap_or("");
            let kind = args["type"].as_str().unwrap_or("");
            let vmid = args["vmid"].as_u64().ok_or("no VM id")?;
            if !(kind == "qemu" || kind == "lxc") || node.is_empty() || !node.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.') {
                return Err("bad VM details".into());
            }
            read(http.post(format!("{base}/api2/json/nodes/{node}/{kind}/{vmid}/status/start")).header("Authorization", auth).timeout(t(20)).send().await.map_err(err)?).await?;
            Ok(json!({"ok": true}))
        }

        // ---------- Portainer (Docker) ----------
        ("portainer", "status") | ("portainer", "test") | ("portainer", "restart") => {
            let key = need(name)?;
            let ep = match cfg.user.trim().parse::<u64>() {
                Ok(n) => n,
                Err(_) => {
                    let v = read(http.get(format!("{base}/api/endpoints")).header("X-API-Key", &key).timeout(t(8)).send().await.map_err(err)?).await?;
                    v.as_array().and_then(|a| a.first()).and_then(|e| e["Id"].as_u64()).ok_or("Portainer has no environments")?
                }
            };
            if action == "restart" {
                let id = args["id"].as_str().unwrap_or("");
                if id.len() < 12 || !id.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Err("bad container id".into());
                }
                read(http.post(format!("{base}/api/endpoints/{ep}/docker/containers/{id}/restart")).header("X-API-Key", &key).timeout(t(70)).send().await.map_err(err)?).await
                    .or_else(|e| if e.contains("EOF") || e.starts_with("HTTP 204") { Ok(json!({})) } else { Err(e) })?;
                return Ok(json!({"ok": true}));
            }
            let v = read(http.get(format!("{base}/api/endpoints/{ep}/docker/containers/json?all=1")).header("X-API-Key", &key).timeout(t(10)).send().await.map_err(err)?).await?;
            let list: Vec<Value> = v.as_array().cloned().unwrap_or_default().iter().map(|c| json!({
                "id": c["Id"], "name": c["Names"][0].as_str().unwrap_or("").trim_start_matches('/'),
                "state": c["State"], "status": c["Status"],
                "unhealthy": c["Status"].as_str().map(|s| s.contains("unhealthy")).unwrap_or(false),
            })).collect();
            Ok(json!({"containers": list}))
        }

        // ---------- Pi-hole (v6 API) ----------
        ("pihole", _) => pihole(st, http, &base, action, args).await,

        // ---------- Jellyfin ----------
        ("jellyfin", "status") | ("jellyfin", "test") => {
            let auth = format!("MediaBrowser Token=\"{}\"", need(name)?);
            let s = read(http.get(format!("{base}/Sessions?activeWithinSeconds=600")).header("Authorization", &auth).timeout(t(8)).send().await.map_err(err)?).await?;
            let playing: Vec<Value> = s.as_array().cloned().unwrap_or_default().iter().filter(|x| x["NowPlayingItem"].is_object()).map(|x| {
                let it = &x["NowPlayingItem"];
                let title = match it["SeriesName"].as_str() { Some(sn) => format!("{sn} · {}", it["Name"].as_str().unwrap_or("")), None => it["Name"].as_str().unwrap_or("").to_string() };
                json!({"user": x["UserName"], "title": title, "paused": x["PlayState"]["IsPaused"] == true, "device": x["DeviceName"]})
            }).collect();
            let l = read(http.get(format!("{base}/Items?Recursive=true&SortBy=DateCreated&SortOrder=Descending&IncludeItemTypes=Movie,Episode&Limit=8")).header("Authorization", &auth).timeout(t(8)).send().await.map_err(err)?).await?;
            let latest: Vec<Value> = l["Items"].as_array().cloned().unwrap_or_default().iter().map(|it| {
                let title = match it["SeriesName"].as_str() { Some(sn) => format!("{sn} · {}", it["Name"].as_str().unwrap_or("")), None => it["Name"].as_str().unwrap_or("").to_string() };
                json!({"id": it["Id"], "title": title})
            }).collect();
            Ok(json!({"playing": playing, "latest": latest}))
        }

        // ---------- Radarr / Sonarr ----------
        ("radarr", "status") | ("radarr", "test") | ("sonarr", "status") | ("sonarr", "test") => {
            let key = need(name)?;
            let tv = name == "sonarr";
            let inc = if tv { "includeSeries=true&includeEpisode=true" } else { "includeMovie=true" };
            let get = |path: String| http.get(format!("{base}{path}")).header("X-Api-Key", key.clone()).timeout(t(10)).send();
            let title_of = |r: &Value| -> String {
                if tv {
                    let ep = if r["episode"].is_object() { &r["episode"] } else { r };
                    format!("{} S{:02}E{:02}", r["series"]["title"].as_str().unwrap_or("?"), ep["seasonNumber"].as_u64().unwrap_or(0), ep["episodeNumber"].as_u64().unwrap_or(0))
                } else {
                    r["movie"]["title"].as_str().or(r["title"].as_str()).unwrap_or("?").to_string()
                }
            };
            let q = read(get(format!("/api/v3/queue?pageSize=20&{inc}")).await.map_err(err)?).await?;
            let queue: Vec<Value> = q["records"].as_array().cloned().unwrap_or_default().iter().map(|r| {
                let size = r["size"].as_f64().unwrap_or(0.0);
                let left = r["sizeleft"].as_f64().unwrap_or(0.0);
                json!({"title": title_of(r), "progress": if size > 0.0 { (1.0 - left / size) * 100.0 } else { 0.0 }, "timeleft": r["timeleft"]})
            }).collect();
            let cal_inc = if tv { "&includeSeries=true" } else { "" };
            let c = read(get(format!("/api/v3/calendar?start={}&end={}{cal_inc}", ymd(0), ymd(7))).await.map_err(err)?).await?;
            let upcoming: Vec<Value> = c.as_array().cloned().unwrap_or_default().iter().take(12).map(|r| {
                let date = if tv { r["airDateUtc"].as_str() } else { r["digitalRelease"].as_str().or(r["physicalRelease"].as_str()).or(r["inCinemas"].as_str()) };
                let title = if tv { format!("{} S{:02}E{:02}", r["series"]["title"].as_str().unwrap_or("?"), r["seasonNumber"].as_u64().unwrap_or(0), r["episodeNumber"].as_u64().unwrap_or(0)) } else { r["title"].as_str().unwrap_or("?").to_string() };
                json!({"title": title, "date": date.unwrap_or("").get(..10).unwrap_or("")})
            }).collect();
            let h = read(get(format!("/api/v3/history?page=1&pageSize=15&sortKey=date&sortDirection=descending&eventType=3&{inc}")).await.map_err(err)?).await?;
            let imported: Vec<Value> = h["records"].as_array().cloned().unwrap_or_default().iter().map(|r| json!({"id": r["id"], "title": title_of(r), "date": r["date"]})).collect();
            Ok(json!({"queue": queue, "upcoming": upcoming, "imported": imported}))
        }

        // ---------- qBittorrent ----------
        ("qbittorrent", "status") | ("qbittorrent", "test") => {
            let get = || http.get(format!("{base}/api/v2/torrents/info")).header("Referer", &base).timeout(t(10)).send();
            let mut r = get().await.map_err(err)?;
            if r.status().as_u16() == 403 {
                let pass = secrets::get(name).unwrap_or_default();
                let lr = http.post(format!("{base}/api/v2/auth/login")).header("Referer", &base)
                    .form(&[("username", cfg.user.trim()), ("password", pass.as_str())]).timeout(t(10)).send().await.map_err(err)?;
                let body = lr.text().await.unwrap_or_default();
                if !body.starts_with("Ok") {
                    return Err("qBittorrent refused the user name or password".into());
                }
                r = get().await.map_err(err)?;
            }
            let v = read(r).await?;
            let list: Vec<Value> = v.as_array().cloned().unwrap_or_default().iter().map(|x| json!({
                "hash": x["hash"], "name": x["name"], "progress": x["progress"].as_f64().unwrap_or(0.0) * 100.0,
                "state": x["state"], "dlspeed": x["dlspeed"], "eta": x["eta"],
            })).collect();
            Ok(json!({"torrents": list}))
        }

        // ---------- Stocks (Yahoo Finance quotes, no key) ----------
        ("stocks", "status") | ("stocks", "test") => {
            let syms: Vec<String> = cfg.extra.split(|c: char| c == ',' || c.is_whitespace()).map(|s| s.trim().to_uppercase())
                .filter(|s| !s.is_empty() && s.len() <= 15 && s.chars().all(|c| c.is_ascii_alphanumeric() || ".^=-".contains(c))).take(12).collect();
            if syms.is_empty() {
                return Err("add a few stock symbols in Settings (for example AAPL, MSFT, BTC-USD)".into());
            }
            let mut out = Vec::new();
            for s in syms {
                let url = format!("https://query1.finance.yahoo.com/v8/finance/chart/{}?range=1d&interval=1d", urlencoding::encode(&s));
                let r = st.http.get(url).header("User-Agent", "Mozilla/5.0 (Macintosh)").timeout(t(8)).send().await;
                let Ok(r) = r else { out.push(json!({"symbol": s, "error": "no answer"})); continue };
                let v: Value = r.json().await.unwrap_or(json!({}));
                let m = &v["chart"]["result"][0]["meta"];
                let price = m["regularMarketPrice"].as_f64();
                let prev = m["chartPreviousClose"].as_f64().or(m["previousClose"].as_f64());
                match (price, prev) {
                    (Some(p), Some(pc)) if pc > 0.0 => out.push(json!({"symbol": s, "name": m["shortName"].as_str().or(m["longName"].as_str()).unwrap_or(&s),
                        "price": p, "change": (p - pc) / pc * 100.0, "currency": m["currency"]})),
                    _ => out.push(json!({"symbol": s, "error": "unknown symbol"})),
                }
            }
            Ok(json!({"quotes": out, "alert": cfg.user.trim().parse::<f64>().unwrap_or(3.0)}))
        }

        // ---------- Weather (Open-Meteo, no key) ----------
        ("weather", "status") | ("weather", "test") => {
            let place = cfg.extra.trim().to_string();
            if place.is_empty() {
                return Err("type your town in Settings".into());
            }
            let cached = st.geo.lock().clone().filter(|g| g.0 == place);
            let (lat, lon, town) = match cached {
                Some(g) => (g.1, g.2, g.3),
                None => {
                    let url = format!("https://geocoding-api.open-meteo.com/v1/search?count=1&name={}", urlencoding::encode(&place));
                    let g = read(st.http.get(url).timeout(t(8)).send().await.map_err(err)?).await?;
                    let r = &g["results"][0];
                    let (Some(la), Some(lo)) = (r["latitude"].as_f64(), r["longitude"].as_f64()) else { return Err(format!("I couldn't find “{place}”")) };
                    let town = format!("{}{}", r["name"].as_str().unwrap_or(&place), r["country"].as_str().map(|c| format!(", {c}")).unwrap_or_default());
                    *st.geo.lock() = Some((place.clone(), la, lo, town.clone()));
                    (la, lo, town)
                }
            };
            let unit = if cfg.user.trim().eq_ignore_ascii_case("fahrenheit") { "&temperature_unit=fahrenheit&wind_speed_unit=mph" } else { "" };
            let url = format!("https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}&current=temperature_2m,apparent_temperature,weather_code,wind_speed_10m&daily=temperature_2m_max,temperature_2m_min,precipitation_probability_max,weather_code&timezone=auto&forecast_days=2{unit}");
            let w = read(st.http.get(url).timeout(t(8)).send().await.map_err(err)?).await?;
            Ok(json!({"town": town, "current": w["current"], "daily": w["daily"], "units": w["current_units"]}))
        }

        // ---------- Lian Li screen (lianli-dashboard container) ----------
        ("lianli", "test") => {
            let v = read(http.get(format!("{base}/api/clippy")).timeout(t(6)).send().await.map_err(err)?).await
                .map_err(|e| if e.contains("404") || e.to_lowercase().contains("not found") { "the container answers, but doesn't have Clippy support yet (update it)".to_string() } else { e })?;
            Ok(v)
        }
        ("lianli", "feed") => read(http.get(format!("{base}/api/clippy")).timeout(t(6)).send().await.map_err(err)?).await,
        ("lianli", "say") | ("lianli", "restart") | ("lianli", "focus") => {
            let secs = if action == "restart" { 70 } else { 8 };
            read(http.post(format!("{base}/api/clippy/{action}")).json(args).timeout(t(secs)).send().await.map_err(err)?).await
        }
        ("lianli", "mode") => {
            let key = args["mode"].as_str().unwrap_or("");
            if key.is_empty() || key.len() > 40 {
                return Err("bad dashboard".into());
            }
            read(http.post(format!("{base}/api/config")).json(&json!({"mode": key})).timeout(t(8)).send().await.map_err(err)?).await
        }
        _ => Err(format!("{} can't do “{action}”", label(name))),
    }
}

async fn pihole(st: &AppState, http: &reqwest::Client, base: &str, action: &str, args: &Value) -> Result<Value, String> {
    async fn login(http: &reqwest::Client, base: &str) -> Result<Option<String>, String> {
        let Some(pass) = secrets::get("pihole") else { return Ok(None) };
        let v = read(http.post(format!("{base}/api/auth")).json(&json!({"password": pass})).timeout(Duration::from_secs(8)).send().await.map_err(err)?).await?;
        if v["session"]["valid"] != true {
            return Err("Pi-hole refused the password".into());
        }
        Ok(v["session"]["sid"].as_str().map(String::from))
    }
    for attempt in 0..2 {
        let mut sid = st.pihole_sid.lock().clone();
        if sid.is_none() || attempt == 1 {
            sid = login(http, base).await?;
            *st.pihole_sid.lock() = sid.clone();
        }
        let with = |rb: reqwest::RequestBuilder| match &sid { Some(s) => rb.header("X-FTL-SID", s), None => rb };
        let res: Result<Value, String> = async {
            match action {
                "status" | "test" => {
                    let s = read(with(http.get(format!("{base}/api/stats/summary"))).timeout(Duration::from_secs(8)).send().await.map_err(err)?).await?;
                    let b = read(with(http.get(format!("{base}/api/dns/blocking"))).timeout(Duration::from_secs(8)).send().await.map_err(err)?).await?;
                    Ok(json!({"total": s["queries"]["total"], "blocked": s["queries"]["blocked"], "percent": s["queries"]["percent_blocked"],
                        "domains": s["gravity"]["domains_being_blocked"], "blocking": b["blocking"], "timer": b["timer"]}))
                }
                "pause" => {
                    let secs = args["seconds"].as_u64().unwrap_or(300).clamp(30, 3600);
                    read(with(http.post(format!("{base}/api/dns/blocking"))).json(&json!({"blocking": false, "timer": secs})).timeout(Duration::from_secs(8)).send().await.map_err(err)?).await
                }
                "resume" => read(with(http.post(format!("{base}/api/dns/blocking"))).json(&json!({"blocking": true, "timer": null})).timeout(Duration::from_secs(8)).send().await.map_err(err)?).await,
                _ => Err(format!("Pi-hole can't do “{action}”")),
            }
        }.await;
        match res {
            Err(e) if attempt == 0 && e.contains("refused the login") => continue,
            other => return other,
        }
    }
    Err("Pi-hole refused the password".into())
}
