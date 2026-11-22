//! Qwen models: the recommended list (one click to download from Hugging Face), the ones already on this
//! Mac (Clippy's models folder plus any GGUF files the user added), and the resumable downloader.

use futures_util::StreamExt;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::AsyncWriteExt;

pub struct Rec {
    pub id: &'static str,
    pub name: &'static str,
    pub file: &'static str,
    pub url: &'static str,
    pub gb: f32,
    pub min_ram: u32,
    pub note: &'static str,
}

pub const RECOMMENDED: &[Rec] = &[
    Rec {
        id: "qwen3-1.7b", name: "Qwen 3 1.7B", file: "Qwen3-1.7B-Q8_0.gguf",
        url: "https://huggingface.co/Qwen/Qwen3-1.7B-GGUF/resolve/main/Qwen3-1.7B-Q8_0.gguf",
        gb: 1.8, min_ram: 8, note: "Tiny and very fast. Fine for quick questions.",
    },
    Rec {
        id: "qwen3-4b-2507", name: "Qwen 3 4B Instruct", file: "Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
        url: "https://huggingface.co/unsloth/Qwen3-4B-Instruct-2507-GGUF/resolve/main/Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
        gb: 2.5, min_ram: 8, note: "Best start for most Macs: quick and surprisingly good.",
    },
    Rec {
        id: "qwen3-8b", name: "Qwen 3 8B", file: "Qwen3-8B-Q4_K_M.gguf",
        url: "https://huggingface.co/Qwen/Qwen3-8B-GGUF/resolve/main/Qwen3-8B-Q4_K_M.gguf",
        gb: 5.0, min_ram: 16, note: "Smarter answers. Good on 16 GB Macs.",
    },
    Rec {
        id: "qwen3-14b", name: "Qwen 3 14B", file: "Qwen3-14B-Q4_K_M.gguf",
        url: "https://huggingface.co/Qwen/Qwen3-14B-GGUF/resolve/main/Qwen3-14B-Q4_K_M.gguf",
        gb: 9.0, min_ram: 24, note: "Noticeably better writing. Needs 24 GB or more.",
    },
    Rec {
        id: "qwen3-30b-a3b-2507", name: "Qwen 3 30B-A3B Instruct", file: "Qwen3-30B-A3B-Instruct-2507-Q4_K_M.gguf",
        url: "https://huggingface.co/unsloth/Qwen3-30B-A3B-Instruct-2507-GGUF/resolve/main/Qwen3-30B-A3B-Instruct-2507-Q4_K_M.gguf",
        gb: 18.6, min_ram: 32, note: "The best of the list and still fast. Needs 32 GB or more.",
    },
];

pub fn dir() -> PathBuf {
    let d = crate::data_dir().join("models");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn ram_gb() -> u32 {
    std::process::Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse::<u64>().ok())
        .map(|b| (b / 1_073_741_824) as u32)
        .unwrap_or(8)
}

fn pretty(file: &str) -> String {
    RECOMMENDED.iter().find(|r| r.file == file).map(|r| r.name.to_string()).unwrap_or_else(|| file.trim_end_matches(".gguf").to_string())
}

/// Everything Clippy can chat with: its own downloads plus files the user added.
pub fn installed(extra: &[String]) -> Vec<Value> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir()) {
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|x| x == "gguf").unwrap_or(false)).collect();
        files.sort();
        for p in files {
            let f = p.file_name().unwrap().to_string_lossy().to_string();
            let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            out.push(json!({"path": p, "name": pretty(&f), "file": f, "bytes": size, "added": false}));
        }
    }
    for e in extra {
        let p = Path::new(e);
        if p.exists() {
            let f = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
            out.push(json!({"path": p, "name": pretty(&f), "file": f, "bytes": size, "added": true}));
        }
    }
    out
}

pub fn recommended() -> Value {
    let ram = ram_gb();
    let have: Vec<String> = installed(&[]).iter().filter_map(|m| m["file"].as_str().map(String::from)).collect();
    let partial = |f: &str| std::fs::metadata(dir().join(format!("{f}.part"))).map(|m| m.len()).unwrap_or(0);
    // Pick the biggest model this Mac runs comfortably as the suggestion.
    let best = RECOMMENDED.iter().filter(|r| r.min_ram <= ram && r.min_ram < 32).last().map(|r| r.id).unwrap_or("qwen3-1.7b");
    json!({
        "ram": ram,
        "list": RECOMMENDED.iter().map(|r| json!({
            "id": r.id, "name": r.name, "file": r.file, "gb": r.gb, "minRam": r.min_ram, "note": r.note,
            "fits": r.min_ram <= ram, "suggested": r.id == best, "have": have.iter().any(|h| h == r.file),
            "partial": partial(r.file),
        })).collect::<Vec<_>>()
    })
}

/// Download one recommended model (resumes a half-finished download). Calls progress(done, total).
pub async fn download(http: &reqwest::Client, id: &str, cancel: &AtomicBool, mut progress: impl FnMut(u64, u64)) -> Result<PathBuf, String> {
    let r = RECOMMENDED.iter().find(|r| r.id == id).ok_or("Unknown model")?;
    let target = dir().join(r.file);
    if target.exists() {
        return Ok(target);
    }
    let part = dir().join(format!("{}.part", r.file));
    let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let mut req = http.get(r.url).header("User-Agent", "Clippy-Mac/1.1");
    if have > 0 {
        req = req.header("Range", format!("bytes={have}-"));
    }
    let resp = req.send().await.map_err(|e| format!("Couldn't reach Hugging Face: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("Hugging Face said {status}"));
    }
    let resumed = status.as_u16() == 206;
    let mut done = if resumed { have } else { 0 };
    let total = resp.content_length().map(|l| l + done).unwrap_or((r.gb * 1e9) as u64);
    let mut f = tokio::fs::OpenOptions::new().create(true).write(true).append(resumed).truncate(!resumed).open(&part).await.map_err(|e| e.to_string())?;
    let mut stream = resp.bytes_stream();
    let mut last = std::time::Instant::now();
    while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::SeqCst) {
            let _ = f.flush().await;
            return Err("cancelled".into());
        }
        let chunk = chunk.map_err(|e| format!("The download stopped: {e}. Click Download again to carry on."))?;
        f.write_all(&chunk).await.map_err(|e| e.to_string())?;
        done += chunk.len() as u64;
        if last.elapsed().as_millis() > 300 {
            last = std::time::Instant::now();
            progress(done, total);
        }
    }
    f.flush().await.map_err(|e| e.to_string())?;
    drop(f);
    if done < total {
        return Err("The download ended early. Click Download again to carry on.".into());
    }
    // A GGUF file starts with the bytes "GGUF".
    let mut head = [0u8; 4];
    if let Ok(mut file) = std::fs::File::open(&part) {
        let _ = std::io::Read::read_exact(&mut file, &mut head);
    }
    if &head != b"GGUF" {
        let _ = std::fs::remove_file(&part);
        return Err("The downloaded file isn't a model file. Try again later.".into());
    }
    std::fs::rename(&part, &target).map_err(|e| e.to_string())?;
    progress(total, total);
    Ok(target)
}
