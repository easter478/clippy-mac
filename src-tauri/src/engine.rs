//! Clippy's own chat engine: llama.cpp (Metal) on one thread, running a Qwen GGUF file from disk.
//! One model is in memory at a time; asking with another file (or chat memory size) swaps it.
//! The model is freed after a few idle minutes so it doesn't sit in memory all day.
//! The model and its context are locals inside `serve`, so the context can borrow the model.

use llama_cpp_2::context::params::{KvCacheType, LlamaContextParams};
use llama_cpp_2::context::LlamaContext;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::LlamaModel;
use llama_cpp_2::sampling::LlamaSampler;
use llama_cpp_2::token::LlamaToken;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const N_BATCH: usize = 2048;

#[derive(Clone, Serialize, Deserialize)]
pub struct Msg {
    pub role: String, // user | assistant
    pub content: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Key {
    pub path: PathBuf,
    pub n_ctx: u32,
}

pub struct GenReq {
    pub key: Key,
    pub system: String,
    pub messages: Vec<Msg>,
    pub temperature: f32,
    pub max_tokens: u32,
}

pub enum Ev {
    Piece(String),
    Done { text: String, tps: f32, tokens: u32, stopped: bool, trimmed: u32 },
    Error(String),
}

pub type Sink = Box<dyn FnMut(Ev) + Send>;
pub type Notify = Arc<dyn Fn(Value) + Send + Sync>;

enum Cmd {
    Generate { req: GenReq, sink: Sink },
    Unload { done: Sender<()> },
}

#[derive(Clone, Default, Serialize)]
pub struct Status {
    pub loaded: Option<String>,
    pub loading: Option<String>,
    pub busy: bool,
}

pub struct Engine {
    tx: Sender<Cmd>,
    pub cancel: Arc<AtomicBool>,
    pub status: Arc<Mutex<Status>>,
    /// Free the model after this many idle seconds (0 = keep it).
    pub idle_secs: Arc<AtomicU64>,
}

impl Engine {
    pub fn start(notify: Notify) -> Engine {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let status = Arc::new(Mutex::new(Status::default()));
        let idle_secs = Arc::new(AtomicU64::new(600));
        let (c, s, i) = (cancel.clone(), status.clone(), idle_secs.clone());
        std::thread::Builder::new()
            .name("llama".into())
            .stack_size(16 << 20)
            .spawn(move || thread_main(rx, c, s, i, notify))
            .expect("engine thread");
        Engine { tx, cancel, status, idle_secs }
    }

    pub fn generate(&self, req: GenReq, sink: Sink) {
        self.cancel.store(false, Ordering::SeqCst);
        let _ = self.tx.send(Cmd::Generate { req, sink });
    }

    pub fn unload(&self) {
        let (tx, rx) = channel();
        self.cancel.store(true, Ordering::SeqCst);
        if self.tx.send(Cmd::Unload { done: tx }).is_ok() {
            let _ = rx.recv_timeout(Duration::from_secs(30));
        }
    }

    pub fn stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
}

fn name_of(p: &std::path::Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}

fn thread_main(rx: Receiver<Cmd>, cancel: Arc<AtomicBool>, status: Arc<Mutex<Status>>, idle: Arc<AtomicU64>, notify: Notify) {
    if std::env::var("CLIPPY_LLAMA_LOGS").is_err() {
        llama_cpp_2::send_logs_to_tracing(llama_cpp_2::LogOptions::default().with_logs_enabled(false));
    }
    let backend = match LlamaBackend::init() {
        Ok(b) => b,
        Err(e) => {
            notify(json!({"kind": "error", "message": format!("llama.cpp didn't start: {e}")}));
            return;
        }
    };
    let mut pending: Option<Cmd> = None;
    loop {
        let cmd = match pending.take() {
            Some(c) => c,
            None => match rx.recv() {
                Ok(c) => c,
                Err(_) => return,
            },
        };
        let key = match &cmd {
            Cmd::Unload { done } => {
                let _ = done.send(());
                continue;
            }
            Cmd::Generate { req, .. } => req.key.clone(),
        };
        let next = serve(&backend, key, cmd, &rx, &cancel, &status, &idle, &notify);
        *status.lock().unwrap() = Status::default();
        notify(json!({"kind": "unloaded"}));
        match next {
            Some(Some(c)) => pending = Some(c),
            Some(None) => {}
            None => return,
        }
    }
}

/// Some(Some(cmd)) = another model was asked for; Some(None) = unloaded; None = shutting down.
#[allow(clippy::too_many_arguments)]
fn serve(backend: &LlamaBackend, key: Key, first: Cmd, rx: &Receiver<Cmd>, cancel: &AtomicBool, status: &Mutex<Status>, idle: &AtomicU64, notify: &Notify) -> Option<Option<Cmd>> {
    let name = name_of(&key.path);
    let fail = |first: Cmd, msg: String| {
        if let Cmd::Generate { mut sink, .. } = first {
            sink(Ev::Error(msg));
        }
        Some(None)
    };
    if !key.path.exists() {
        return fail(first, format!("The model file {name} isn't on this Mac any more. Pick or download one in Settings."));
    }
    status.lock().unwrap().loading = Some(name.clone());
    notify(json!({"kind": "loading", "model": name}));
    let mparams = LlamaModelParams::default().with_n_gpu_layers(999);
    let model = match LlamaModel::load_from_file(backend, &key.path, &mparams) {
        Ok(m) => m,
        Err(e) => return fail(first, format!("Couldn't load {name}: {e}")),
    };
    let arch = model.meta_val_str("general.architecture").unwrap_or_default();
    // Qwen3 hybrid models think first unless told not to; the 2507 Instruct models never think.
    let skip_think = arch.starts_with("qwen3") && !name.to_lowercase().contains("instruct-2507");
    let cparams = LlamaContextParams::default()
        .with_n_ctx(NonZeroU32::new(key.n_ctx))
        .with_n_batch(N_BATCH as u32)
        .with_n_ubatch(512)
        .with_flash_attention_policy(llama_cpp_sys_2::LLAMA_FLASH_ATTN_TYPE_ENABLED)
        .with_type_k(KvCacheType::Q8_0)
        .with_type_v(KvCacheType::Q8_0);
    let mut ctx = match model.new_context(backend, cparams) {
        Ok(c) => c,
        Err(e) => return fail(first, format!("Not enough memory for {name} ({e}). Try a smaller model or chat memory in Settings.")),
    };
    *status.lock().unwrap() = Status { loaded: Some(name.clone()), loading: None, busy: false };
    notify(json!({"kind": "loaded", "model": name}));

    let mut cached: Vec<LlamaToken> = Vec::new();
    let mut cmd = Some(first);
    loop {
        let c = match cmd.take() {
            Some(c) => c,
            None => {
                let secs = idle.load(Ordering::Relaxed);
                let r = if secs == 0 { rx.recv().map_err(|_| RecvTimeoutError::Disconnected) } else { rx.recv_timeout(Duration::from_secs(secs)) };
                match r {
                    Ok(c) => c,
                    Err(RecvTimeoutError::Timeout) => return Some(None), // idle: free the memory
                    Err(RecvTimeoutError::Disconnected) => return None,
                }
            }
        };
        match c {
            Cmd::Unload { done } => {
                drop(ctx);
                drop(model);
                let _ = done.send(());
                return Some(None);
            }
            Cmd::Generate { req, mut sink } if req.key == key => {
                status.lock().unwrap().busy = true;
                let r = generate(&model, &mut ctx, &mut cached, skip_think, req, &mut sink, cancel);
                status.lock().unwrap().busy = false;
                if let Err(e) = r {
                    cached.clear();
                    ctx.clear_kv_cache();
                    sink(Ev::Error(e));
                }
            }
            other => return Some(Some(other)),
        }
    }
}

fn render(system: &str, msgs: &[Msg], skip_think: bool) -> String {
    let mut t = String::new();
    if !system.trim().is_empty() {
        t.push_str(&format!("<|im_start|>system\n{}<|im_end|>\n", system.trim()));
    }
    for m in msgs {
        let role = if m.role == "user" { "user" } else { "assistant" };
        t.push_str(&format!("<|im_start|>{role}\n{}<|im_end|>\n", m.content.trim()));
    }
    t.push_str("<|im_start|>assistant\n");
    if skip_think {
        t.push_str("<think>\n\n</think>\n\n");
    }
    t
}

/// Streams UTF-8 safely: a token can end in the middle of a multi-byte character.
#[derive(Default)]
struct Utf8Stream {
    pending: Vec<u8>,
}

impl Utf8Stream {
    fn push(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        match std::str::from_utf8(&self.pending) {
            Ok(s) => {
                let out = s.to_string();
                self.pending.clear();
                out
            }
            Err(e) => {
                let v = e.valid_up_to();
                let mut out = String::from_utf8_lossy(&self.pending[..v]).into_owned();
                if e.error_len().is_some() {
                    out.push_str(&String::from_utf8_lossy(&self.pending[v..]));
                    self.pending.clear();
                } else {
                    self.pending.drain(..v);
                }
                out
            }
        }
    }
}

fn generate(model: &LlamaModel, ctx: &mut LlamaContext, cached: &mut Vec<LlamaToken>, skip_think: bool, req: GenReq, sink: &mut Sink, cancel: &AtomicBool) -> Result<(), String> {
    let n_ctx = ctx.n_ctx() as usize;
    let vocab = model.vocab();
    let reserve = (req.max_tokens as usize + 64).min(n_ctx / 2);
    // Drop the oldest turns until the chat fits, leaving room for the reply.
    let mut msgs = req.messages.clone();
    let mut trimmed = 0u32;
    let toks = loop {
        let toks = vocab.tokenize(render(&req.system, &msgs, skip_think).as_bytes(), true, true);
        if toks.len() + reserve <= n_ctx || msgs.len() <= 1 {
            break toks;
        }
        msgs.remove(0);
        trimmed += 1;
        while msgs.len() > 1 && msgs[0].role != "user" {
            msgs.remove(0);
            trimmed += 1;
        }
    };
    if toks.len() + 16 >= n_ctx {
        return Err("That message is too long for the chat memory. Make it shorter or pick a bigger chat memory in Settings.".into());
    }
    // Reuse the unchanged start of the previous prompt (the earlier chat), so long chats stay quick.
    let mut reused = cached.iter().zip(toks.iter()).take_while(|(a, b)| a == b).count().min(toks.len() - 1);
    if reused > 0 {
        if !matches!(ctx.clear_kv_cache_seq(Some(0), Some(reused as u32), None), Ok(true)) {
            ctx.clear_kv_cache();
            reused = 0;
        }
    } else {
        ctx.clear_kv_cache();
    }
    cached.clear();
    let mut batch = LlamaBatch::new(N_BATCH, 1);
    let mut i = reused;
    while i < toks.len() {
        if cancel.load(Ordering::Relaxed) {
            ctx.clear_kv_cache();
            sink(Ev::Done { text: String::new(), tps: 0.0, tokens: 0, stopped: true, trimmed });
            return Ok(());
        }
        batch.clear();
        let end = (i + N_BATCH).min(toks.len());
        for (j, t) in toks.iter().enumerate().take(end).skip(i) {
            batch.add(*t, j as i32, &[0], j == toks.len() - 1).map_err(|e| e.to_string())?;
        }
        ctx.decode(&mut batch).map_err(|e| decode_err("reading the chat", e))?;
        i = end;
    }
    let n_prompt = toks.len();
    *cached = toks;
    let mut n_past = n_prompt as i32;

    let seed = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0) & 0xffff_ffff) as u32;
    let mut smp = LlamaSampler::chain_simple([
        LlamaSampler::top_k(20),
        LlamaSampler::top_p(0.8, 1),
        LlamaSampler::min_p(0.0, 1),
        LlamaSampler::penalties(vocab.n_tokens(), 64, 1.05, 0.0, 0.0),
        LlamaSampler::temp(req.temperature),
        LlamaSampler::dist(seed),
    ]);
    let mut one = LlamaBatch::new(1, 1);
    let mut utf8 = Utf8Stream::default();
    let mut text = String::new();
    let t_gen = Instant::now();
    let mut n_gen = 0u32;
    let mut stopped = false;
    loop {
        if cancel.load(Ordering::Relaxed) {
            stopped = true;
            break;
        }
        let tok = smp.sample(ctx, -1);
        if vocab.is_eog(tok) {
            break;
        }
        n_gen += 1;
        let piece = utf8.push(&vocab.token_to_piece(tok, false, None));
        if !piece.is_empty() {
            text.push_str(&piece);
            sink(Ev::Piece(piece));
        }
        if n_past as usize + 1 >= n_ctx || n_gen >= req.max_tokens {
            break;
        }
        one.clear();
        one.add(tok, n_past, &[0], true).map_err(|e| e.to_string())?;
        ctx.decode(&mut one).map_err(|e| decode_err("writing", e))?;
        n_past += 1;
        cached.push(tok);
    }
    let ms = t_gen.elapsed().as_millis() as f32;
    let tps = if ms > 0.0 && n_gen > 1 { (n_gen - 1) as f32 * 1000.0 / ms } else { 0.0 };
    sink(Ev::Done { text: strip_think(&text), tps, tokens: n_gen, stopped, trimmed });
    Ok(())
}

fn decode_err(when: &str, e: impl std::fmt::Display) -> String {
    let e = e.to_string();
    if e.contains("-3") || e.contains("-2") {
        format!("The Mac ran out of graphics memory while {when}. Close other big apps or pick a smaller model, then try again.")
    } else {
        format!("The model stopped while {when}: {e}")
    }
}

/// Remove a "<think>…</think>" block if a model wrote one anyway.
pub fn strip_think(text: &str) -> String {
    let t = text.trim_start();
    if let Some(rest) = t.strip_prefix("<think>") {
        if let Some(i) = rest.find("</think>") {
            return rest[i + 8..].trim().to_string();
        }
        return String::new();
    }
    text.trim().to_string()
}
