//! Custom actions: your own Deck tiles that call any HTTP service on your network, described in a JSON
//! file (custom-actions.json in Clippy's data folder; see examples/custom-actions.example.json).
//!
//! Placeholders in url, headers, body and the result texts:
//!   {{input}}            the text or link the user typed
//!   {{file}}             (body only, alone) the dropped/chosen file's bytes
//!   {{file.name}} {{file.size}}
//!   {{secret:NAME}}      a value from the Keychain, set in Settings → Custom actions (never in the file)
//!   {{response.a.b.0}}   a field from the JSON answer (result texts; in a watch's done_text, the job itself)
//! Values are URL-encoded inside `url` and JSON-escaped inside a JSON body.

use crate::secrets;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Watch {
    pub url: String,
    pub list: String,
    pub id: String,
    pub ids: String,
    pub status: String,
    pub title: String,
    pub done: Vec<String>,
    pub failed: Vec<String>,
    pub every: u64,
    pub done_text: String,
}

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Action {
    pub name: String,
    pub icon: String,
    /// none | text | url | file
    pub input: String,
    pub prompt: String,
    pub placeholder: String,
    /// Run this action when a file is dropped on Clippy.
    pub drop: bool,
    pub method: String,
    pub url: String,
    pub headers: BTreeMap<String, String>,
    pub body: String,
    pub result: String,
    pub copy: String,
    pub open: String,
    pub max_mb: u64,
    pub timeout: u64,
    pub watch: Option<Watch>,
}

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct File {
    pub actions: Vec<Action>,
}

pub fn default_path() -> PathBuf {
    crate::data_dir().join("custom-actions.json")
}

pub fn load(path: &Path) -> Result<File, String> {
    if !path.exists() {
        return Ok(File::default());
    }
    let b = std::fs::read(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
    let f: File = serde_json::from_slice(&b).map_err(|e| format!("{} isn't valid: {e}", path.display()))?;
    for a in &f.actions {
        if !(a.url.starts_with("http://") || a.url.starts_with("https://")) {
            return Err(format!("“{}”: url must start with http:// or https://", a.name));
        }
    }
    Ok(f)
}

/// Secret names a file uses, so Settings can show a "Set…" button for each.
pub fn secret_names(f: &File) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for a in &f.actions {
        let mut all = vec![a.url.clone(), a.body.clone()];
        all.extend(a.headers.values().cloned());
        if let Some(w) = &a.watch {
            all.push(w.url.clone());
        }
        for s in all {
            let mut rest = s.as_str();
            while let Some(i) = rest.find("{{secret:") {
                let tail = &rest[i + 9..];
                if let Some(j) = tail.find("}}") {
                    let n = tail[..j].to_string();
                    if !out.contains(&n) {
                        out.push(n);
                    }
                    rest = &tail[j..];
                } else {
                    break;
                }
            }
        }
    }
    out
}

/// What the windows may see: no headers or bodies (they can carry secret placeholders).
pub fn summary(f: &File) -> Value {
    json!(f.actions.iter().enumerate().map(|(i, a)| json!({
        "index": i, "name": a.name, "icon": if a.icon.is_empty() { "bolt" } else { a.icon.as_str() },
        "input": if a.input.is_empty() { "none" } else { a.input.as_str() },
        "prompt": a.prompt, "placeholder": a.placeholder, "drop": a.drop, "watch": a.watch.as_ref().map(|w| w.every.max(5)),
    })).collect::<Vec<_>>())
}

fn pick<'a>(v: &'a Value, path: &str) -> &'a Value {
    let mut cur = v;
    for part in path.split('.').filter(|p| !p.is_empty()) {
        cur = match part.parse::<usize>() {
            Ok(i) if cur.is_array() => &cur[i],
            _ => &cur[part],
        };
    }
    cur
}

fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Esc {
    Raw,
    Url,
    Json,
}

struct Ctx<'a> {
    input: &'a str,
    file_name: &'a str,
    file_size: u64,
    response: &'a Value,
}

fn render(t: &str, c: &Ctx, esc: Esc) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = t;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 2..];
        let j = tail.find("}}").ok_or("a {{ is missing its }}")?;
        let key = tail[..j].trim();
        let val = if key == "input" {
            c.input.to_string()
        } else if key == "file.name" {
            c.file_name.to_string()
        } else if key == "file.size" {
            c.file_size.to_string()
        } else if let Some(n) = key.strip_prefix("secret:") {
            secrets::get(n).ok_or_else(|| format!("the secret “{n}” isn't set (Settings → Custom actions)"))?
        } else if let Some(p) = key.strip_prefix("response.") {
            as_text(pick(c.response, p))
        } else if key == "response" {
            as_text(c.response)
        } else {
            return Err(format!("unknown placeholder {{{{{key}}}}}"));
        };
        out.push_str(&match esc {
            Esc::Raw => val.replace(['\r', '\n'], " "),
            Esc::Url => urlencoding::encode(&val).into_owned(),
            Esc::Json => serde_json::to_string(&val).map(|s| s[1..s.len() - 1].to_string()).unwrap_or_default(),
        });
        rest = &tail[j + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

fn request(http: &reqwest::Client, method: &str, url: &str, headers: &BTreeMap<String, String>, c: &Ctx) -> Result<reqwest::RequestBuilder, String> {
    let m = match method.to_uppercase().as_str() {
        "" | "GET" => reqwest::Method::GET,
        "POST" => reqwest::Method::POST,
        "PUT" => reqwest::Method::PUT,
        "PATCH" => reqwest::Method::PATCH,
        "DELETE" => reqwest::Method::DELETE,
        other => return Err(format!("method {other} isn't supported")),
    };
    let mut rb = http.request(m, render(url, c, Esc::Url)?);
    for (k, v) in headers {
        rb = rb.header(k.as_str(), render(v, c, Esc::Raw)?);
    }
    Ok(rb)
}

pub async fn run(http: &reqwest::Client, a: &Action, input: &str, file: Option<&str>) -> Result<Value, String> {
    let (fname, fsize, fpath) = match file {
        Some(p) => {
            let pb = PathBuf::from(p);
            let meta = tokio::fs::metadata(&pb).await.map_err(|e| format!("can't read that file ({e})"))?;
            if meta.is_dir() {
                return Err("that's a folder; use a file (or zip the folder first)".into());
            }
            let max = if a.max_mb > 0 { a.max_mb } else { 2048 } * 1_048_576;
            if meta.len() > max {
                return Err(format!("that file is bigger than {} MB", max / 1_048_576));
            }
            (pb.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "file".into()), meta.len(), Some(pb))
        }
        None => (String::new(), 0, None),
    };
    if a.input == "file" && fpath.is_none() {
        return Err("pick a file first".into());
    }
    if a.input == "url" && !(input.starts_with("http://") || input.starts_with("https://")) {
        return Err("that isn't a web link".into());
    }
    let empty = json!({});
    let c = Ctx { input, file_name: &fname, file_size: fsize, response: &empty };
    let mut rb = request(http, &a.method, &a.url, &a.headers, &c)?;
    let json_body = a.headers.iter().any(|(k, v)| k.eq_ignore_ascii_case("content-type") && v.contains("json"));
    if a.body.trim() == "{{file}}" {
        let p = fpath.as_ref().ok_or("this action needs a file")?;
        let f = tokio::fs::File::open(p).await.map_err(|e| e.to_string())?;
        rb = rb.header("Content-Length", fsize).body(reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(f)));
    } else if !a.body.is_empty() {
        rb = rb.body(render(&a.body, &c, if json_body { Esc::Json } else { Esc::Raw })?);
    }
    let r = rb.timeout(Duration::from_secs(if a.timeout > 0 { a.timeout } else if fpath.is_some() { 900 } else { 20 })).send().await.map_err(crate::integrations::err)?;
    let code = r.status();
    let text = r.text().await.unwrap_or_default();
    let resp: Value = serde_json::from_str(&text).unwrap_or_else(|_| Value::String(text.chars().take(500).collect()));
    if !code.is_success() {
        let m = resp.get("error").or_else(|| resp.get("message")).map(as_text).unwrap_or_else(|| format!("HTTP {}", code.as_u16()));
        return Err(m);
    }
    let c = Ctx { input, file_name: &fname, file_size: fsize, response: &resp };
    let message = if a.result.is_empty() { "Done.".to_string() } else { render(&a.result, &c, Esc::Raw)? };
    let copy = if a.copy.is_empty() { String::new() } else { render(&a.copy, &c, Esc::Raw)? };
    let open = if a.open.is_empty() { String::new() } else { render(&a.open, &c, Esc::Raw)? };
    let ids: Vec<String> = match &a.watch {
        Some(w) if !w.ids.is_empty() => {
            let v = pick(&resp, w.ids.trim_start_matches("response."));
            match v.as_array() {
                Some(arr) => arr.iter().map(as_text).collect(),
                None if !v.is_null() => vec![as_text(v)],
                None => vec![],
            }
        }
        _ => vec![],
    };
    Ok(json!({"message": message, "copy": copy, "open": open, "watch": ids}))
}

/// One look at a watched job list: [{id, title, state: running|done|failed}] for the given ids.
pub async fn watch(http: &reqwest::Client, a: &Action, ids: &[String]) -> Result<Value, String> {
    let w = a.watch.as_ref().ok_or("this action has nothing to watch")?;
    let empty = json!({});
    let c = Ctx { input: "", file_name: "", file_size: 0, response: &empty };
    let r = request(http, "GET", &w.url, &a.headers, &c)?.timeout(Duration::from_secs(10)).send().await.map_err(crate::integrations::err)?;
    let v: Value = r.json().await.map_err(|e| e.to_string())?;
    let list = pick(&v, &w.list).as_array().cloned().unwrap_or_default();
    let mut out = Vec::new();
    for item in list {
        let id = as_text(pick(&item, &w.id));
        if !ids.contains(&id) {
            continue;
        }
        let status = as_text(pick(&item, &w.status));
        let state = if w.done.contains(&status) { "done" } else if w.failed.contains(&status) { "failed" } else { "running" };
        let title = as_text(pick(&item, &w.title));
        let ctx = Ctx { input: "", file_name: "", file_size: 0, response: &item };
        let text = if state == "done" && !w.done_text.is_empty() { render(&w.done_text, &ctx, Esc::Raw).unwrap_or_default() } else { String::new() };
        out.push(json!({"id": id, "title": title, "state": state, "status": status, "text": text}));
    }
    Ok(json!(out))
}
