//! Clippy: a desktop paperclip buddy for macOS with a built-in local Qwen chat (llama.cpp on Metal) and
//! optional integrations. Rust does the I/O (settings, Keychain, network, the model, the Swift helper,
//! windows and the menu bar); the buddy window's JavaScript (ui/buddy.js) decides what Clippy says.
mod assist;
mod cli;
mod custom;
mod engine;
mod integrations;
mod models;
mod secrets;

use engine::{Ev, GenReq, Key, Msg};
use integrations::Integ;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::ipc::Channel;
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, WebviewWindow};
use tauri_plugin_clipboard_manager::ClipboardExt;

const DECK_W: f64 = 224.0;
const DECK_TAB: f64 = 30.0;

#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Service {
    pub name: String,
    pub url: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub personality: String,      // normal | sarcastic
    pub model: String,            // path of the GGUF file Clippy chats with
    pub model_files: Vec<String>, // GGUF files the user added from elsewhere
    pub ctx: u32,                 // chat memory in tokens
    pub unload_minutes: u32,      // free the model after this many idle minutes (0 = never)
    pub web_lookup: bool,         // look general questions up on Wikipedia
    pub chat_on_top: bool,        // keep the chat window above other apps' windows
    pub kinds: BTreeMap<String, bool>,
    pub quiet_two: bool,
    pub ignores: BTreeMap<String, u32>,
    pub sleep_minutes: u32,
    pub buddy: bool,
    pub deck_mode: String, // always | click | never
    pub deck_collapsed: bool,
    pub buddy_pos: Option<(f64, f64)>,
    pub size: String,
    pub integrations: BTreeMap<String, Integ>,
    pub checks_on: bool,
    pub checks: Vec<Service>,
    pub screen_seconds: u32,
    pub mirror_to_screen: bool,
}

pub const KINDS: &[&str] = &["homelab", "downloads", "media", "stocks", "tips", "health"];

impl Default for Settings {
    fn default() -> Self {
        Settings {
            personality: "normal".into(),
            model: String::new(),
            model_files: vec![],
            ctx: 8192,
            unload_minutes: 10,
            web_lookup: true,
            chat_on_top: true,
            kinds: KINDS.iter().map(|k| (k.to_string(), true)).collect(),
            quiet_two: true,
            ignores: BTreeMap::new(),
            sleep_minutes: 5,
            buddy: true,
            deck_mode: "always".into(),
            deck_collapsed: false,
            buddy_pos: None,
            size: "medium".into(),
            integrations: BTreeMap::new(),
            checks_on: false,
            checks: vec![],
            screen_seconds: 8,
            mirror_to_screen: true,
        }
    }
}

pub struct AppState {
    pub settings: Mutex<Settings>,
    helper_in: Mutex<Option<std::process::ChildStdin>>,
    pub http: reqwest::Client,
    /// Only for integrations where the user ticked "self-signed certificate".
    pub http_insecure: reqwest::Client,
    menu: Mutex<Option<MenuRefs>>,
    tray: Mutex<Option<tauri::tray::TrayIcon>>,
    engine: engine::Engine,
    /// A short status summary from the buddy (health, homelab, weather…) that the chat can see.
    context: Mutex<String>,
    dl_cancel: Arc<AtomicBool>,
    downloading: Mutex<Option<String>>,
    pub pihole_sid: Mutex<Option<String>>,
    pub geo: Mutex<Option<(String, f64, f64, String)>>,
    deck_open: AtomicBool,
    /// Height the Deck's tiles need (set by the Deck itself).
    deck_h: Mutex<f64>,
    /// "Open Settings" was asked for; the chat page reads and clears it when it loads.
    want_settings: AtomicBool,
}

struct MenuRefs {
    buddy: CheckMenuItem<tauri::Wry>,
}

pub fn data_dir() -> PathBuf {
    let dir = dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("app.clippy.desktop");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

fn load_settings() -> Settings {
    let raw: Value = std::fs::read(settings_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(json!({}));
    let mut s: Settings = serde_json::from_value(raw.clone()).unwrap_or_default();
    // Settings from Clippy 1.0: keep the screen address, the service checks and the upload token.
    if raw.get("integrations").is_none() {
        if let Some(u) = raw["lianli"].as_str() {
            s.integrations.insert("lianli".into(), Integ { on: raw["mode"] == "connected", url: u.into(), ..Default::default() });
        }
        if let Some(list) = raw["services"].as_array() {
            s.checks = list.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect();
            s.checks_on = !s.checks.is_empty();
        }
        if let Some(o) = raw["owner"].as_str().filter(|o| o.len() >= 16) {
            if !secrets::has("custom:owner") {
                let _ = secrets::set("custom:owner", o);
            }
        }
        if raw["deck"] == false {
            s.deck_mode = "never".into();
        }
    }
    for k in KINDS {
        s.kinds.entry(k.to_string()).or_insert(true);
    }
    for n in integrations::NAMES {
        s.integrations.entry(n.to_string()).or_default();
    }
    s.ctx = s.ctx.clamp(2048, 32768);
    s
}

fn write_settings(s: &Settings) {
    let p = settings_path();
    let tmp = p.with_extension("json.tmp");
    if let Ok(b) = serde_json::to_vec_pretty(s) {
        if std::fs::write(&tmp, b).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

fn helper_path() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_default();
    let side = exe.with_file_name("clippy-helper");
    if side.exists() {
        return side;
    }
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../helper/build/clippy-helper"))
}

// ---------------- settings ----------------
#[tauri::command]
fn get_settings(state: tauri::State<Arc<AppState>>) -> Settings {
    state.settings.lock().clone()
}

#[tauri::command]
fn save_settings(app: AppHandle, state: tauri::State<Arc<AppState>>, settings: Settings) {
    {
        let mut s = state.settings.lock();
        let geo_changed = s.integrations.get("weather").map(|w| w.extra.clone()) != settings.integrations.get("weather").map(|w| w.extra.clone());
        *s = settings;
        s.ctx = s.ctx.clamp(2048, 32768);
        write_settings(&s);
        if geo_changed {
            *state.geo.lock() = None;
        }
    }
    apply_settings(&app, &state);
    let _ = app.emit("settings", state.settings.lock().clone());
}

fn apply_settings(app: &AppHandle, state: &AppState) {
    let s = state.settings.lock().clone();
    state.engine.idle_secs.store(s.unload_minutes as u64 * 60, Ordering::Relaxed);
    if let Some(m) = state.menu.lock().as_ref() {
        let _ = m.buddy.set_checked(s.buddy);
    }
    if let Some(w) = app.get_webview_window("chat").filter(|w| w.is_visible().unwrap_or(false)) {
        let _ = w.set_always_on_top(s.chat_on_top);
    }
    if let Some(w) = app.get_webview_window("buddy") {
        if s.buddy && !w.is_visible().unwrap_or(false) {
            // put him in his saved corner before he shows (his page re-fits him once it has drawn)
            let (ax, ay, aw, ah) = work_area(&w);
            let (right, bottom) = s.buddy_pos.unwrap_or((ax + aw - 250.0, ay + ah - 8.0));
            if let (Ok(sz), Ok(sf)) = (w.outer_size(), w.scale_factor()) {
                let (bw, bh) = (sz.width as f64 / sf, sz.height as f64 / sf);
                let _ = w.set_position(LogicalPosition::new((right - bw).clamp(ax, ax + aw - bw), (bottom - bh).clamp(ay, ay + ah - bh)));
            }
        }
        let _ = if s.buddy { w.show() } else { w.hide() };
    }
    if let Some(w) = app.get_webview_window("deck") {
        let show = s.deck_mode == "always" || (s.deck_mode == "click" && state.deck_open.load(Ordering::Relaxed));
        if show {
            place_deck(&w, s.deck_collapsed && s.deck_mode == "always");
            let _ = w.show();
        } else {
            let _ = w.hide();
        }
    }
}

fn update<F: FnOnce(&mut Settings)>(app: &AppHandle, f: F) {
    let state = app.state::<Arc<AppState>>();
    {
        let mut s = state.settings.lock();
        f(&mut s);
        write_settings(&s);
    }
    apply_settings(app, &state);
    let _ = app.emit("settings", state.settings.lock().clone());
}

// ---------------- windows ----------------
fn work_area(w: &WebviewWindow) -> (f64, f64, f64, f64) {
    if let Ok(Some(m)) = w.primary_monitor().or_else(|_| w.current_monitor()) {
        let sf = m.scale_factor();
        let a = m.work_area();
        return (a.position.x as f64 / sf, a.position.y as f64 / sf, a.size.width as f64 / sf, a.size.height as f64 / sf);
    }
    (0.0, 25.0, 1440.0, 875.0)
}

fn place_deck(w: &WebviewWindow, collapsed: bool) {
    let (x, y, aw, ah) = work_area(w);
    let want = w.app_handle().state::<Arc<AppState>>().deck_h.lock().clamp(200.0, 900.0);
    let h = (ah - 80.0).min(want);
    let width = if collapsed { DECK_TAB } else { DECK_W };
    let _ = w.set_size(LogicalSize::new(width, h));
    let _ = w.set_position(LogicalPosition::new(x + aw - width - 6.0, y + (ah - h) / 2.0 - 20.0));
}

#[tauri::command]
fn buddy_fit(app: AppHandle, state: tauri::State<Arc<AppState>>, width: f64, height: f64) {
    let Some(w) = app.get_webview_window("buddy") else { return };
    let (ax, ay, aw, ah) = work_area(&w);
    let (mut right, mut bottom) = state.settings.lock().buddy_pos.unwrap_or((ax + aw - 250.0, ay + ah - 8.0));
    right = right.clamp(ax + width.min(aw), ax + aw);
    bottom = bottom.clamp(ay + height.min(ah), ay + ah);
    let _ = w.set_size(LogicalSize::new(width, height));
    let _ = w.set_position(LogicalPosition::new(right - width, bottom - height));
}

#[tauri::command]
fn buddy_moved(app: AppHandle) {
    let Some(w) = app.get_webview_window("buddy") else { return };
    let (Ok(pos), Ok(size), Ok(sf)) = (w.outer_position(), w.outer_size(), w.scale_factor()) else { return };
    let right = (pos.x as f64 + size.width as f64) / sf;
    let bottom = (pos.y as f64 + size.height as f64) / sf;
    update(&app, |s| s.buddy_pos = Some((right, bottom)));
}

#[tauri::command]
fn deck_collapse(app: AppHandle, collapsed: bool) {
    let state = app.state::<Arc<AppState>>();
    if state.settings.lock().deck_mode == "click" {
        state.deck_open.store(false, Ordering::Relaxed);
        apply_settings(&app, &state);
        return;
    }
    update(&app, |s| s.deck_collapsed = collapsed);
}

#[tauri::command]
fn deck_fit(app: AppHandle, state: tauri::State<Arc<AppState>>, height: f64) {
    let changed = (*state.deck_h.lock() - height).abs() > 2.0;
    *state.deck_h.lock() = height;
    if changed {
        apply_settings(&app, &state);
    }
}

/// "Show the Deck when I click Clippy" mode: each click on Clippy opens or closes it.
#[tauri::command]
fn deck_toggle(app: AppHandle) -> bool {
    let state = app.state::<Arc<AppState>>();
    let open = !state.deck_open.load(Ordering::Relaxed);
    state.deck_open.store(open, Ordering::Relaxed);
    apply_settings(&app, &state);
    open
}

fn front(app: &AppHandle, w: &WebviewWindow) {
    let _ = w.show();
    // float it so it lands above other apps' windows even when macOS won't switch apps; with
    // "Keep the chat above other windows" it stays floating, otherwise only for a moment
    let _ = w.set_always_on_top(true);
    let keep = w.label() == "chat" && app.state::<Arc<AppState>>().settings.lock().chat_on_top;
    if !keep {
        let w2 = w.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(600));
            let _ = w2.set_always_on_top(false);
        });
    }
    #[cfg(target_os = "macos")]
    {
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Regular);
        let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    }
    let _ = w.unminimize();
    let _ = w.set_focus();
}

#[tauri::command]
fn show_window(app: AppHandle, which: String) {
    match which.as_str() {
        // Settings is a page inside the chat window, so it can never open behind other windows
        "settings" | "chat" => {
            if let Some(w) = app.get_webview_window("chat") {
                if !w.is_visible().unwrap_or(false) {
                    // open the chat just left of Clippy, inside the screen
                    let (ax, ay, aw, ah) = work_area(&w);
                    let (right, bottom) = app.state::<Arc<AppState>>().settings.lock().buddy_pos.unwrap_or((ax + aw - 250.0, ay + ah - 8.0));
                    let (cw, ch) = (440.0, 620.0);
                    let mut x = right - 130.0 - cw;
                    // keep clear of the Deck when it's showing
                    if let Some(d) = app.get_webview_window("deck").filter(|d| d.is_visible().unwrap_or(false)) {
                        if let (Ok(p), Ok(sf)) = (d.outer_position(), d.scale_factor()) {
                            x = x.min(p.x as f64 / sf - cw - 12.0);
                        }
                    }
                    let x = x.clamp(ax + 8.0, (ax + aw - cw - 8.0).max(ax + 8.0));
                    let y = (bottom - ch - 10.0).clamp(ay + 8.0, (ay + ah - ch - 8.0).max(ay + 8.0));
                    let _ = w.set_position(LogicalPosition::new(x, y));
                }
                // if the page is still loading, the eval can be lost: the chat asks for this flag once it's up
                app.state::<Arc<AppState>>().want_settings.store(which == "settings", Ordering::Relaxed);
                let _ = w.eval(if which == "settings" {
                    "if (!location.pathname.endsWith('settings.html')) location.replace('settings.html')"
                } else {
                    "if (location.pathname.endsWith('settings.html')) location.replace('chat.html')"
                });
                front(&app, &w);
            }
        }
        "buddy" => update(&app, |s| s.buddy = true),
        "deck" => {
            let state = app.state::<Arc<AppState>>();
            state.deck_open.store(true, Ordering::Relaxed);
            let mode = state.settings.lock().deck_mode.clone();
            if mode == "never" {
                update(&app, |s| s.deck_mode = "always".into());
            } else {
                update(&app, |s| s.deck_collapsed = false);
            }
        }
        _ => {}
    }
}

#[tauri::command]
fn hide_window(app: AppHandle, which: String) {
    match which.as_str() {
        "buddy" => update(&app, |s| s.buddy = false),
        "deck" => {
            let state = app.state::<Arc<AppState>>();
            state.deck_open.store(false, Ordering::Relaxed);
            if state.settings.lock().deck_mode == "always" {
                update(&app, |s| s.deck_mode = "never".into());
            } else {
                apply_settings(&app, &state);
            }
        }
        "settings" | "chat" => {
            if let Some(w) = app.get_webview_window("chat") {
                let _ = w.hide();
            }
        }
        _ => {}
    }
}

#[tauri::command]
fn set_tray_title(state: tauri::State<Arc<AppState>>, title: String) {
    if let Some(t) = state.tray.lock().as_ref() {
        let _ = t.set_title(if title.is_empty() { None } else { Some(title) });
    }
}

#[tauri::command]
fn quit(app: AppHandle) {
    app.exit(0);
}

// ---------------- integrations ----------------
#[tauri::command]
async fn integ(state: tauri::State<'_, Arc<AppState>>, name: String, action: String, args: Option<Value>) -> Result<Value, String> {
    integrations::call(&state, &name, &action, &args.unwrap_or(json!({}))).await
}

/// Which secrets are set (never their values).
#[tauri::command]
async fn secrets_status(names: Vec<String>) -> BTreeMap<String, bool> {
    names.into_iter().filter(|n| secrets::valid_name(n)).map(|n| {
        let h = secrets::has(&n);
        (n, h)
    }).collect()
}

#[tauri::command]
async fn secret_set(state: tauri::State<'_, Arc<AppState>>, name: String, value: String) -> Result<(), String> {
    if !secrets::valid_name(&name) {
        return Err("bad name".into());
    }
    if name == "pihole" {
        *state.pihole_sid.lock() = None;
    }
    secrets::set(&name, value.trim())
}

#[tauri::command]
async fn check_services(state: tauri::State<'_, Arc<AppState>>) -> Result<Vec<Value>, String> {
    let list = state.settings.lock().checks.clone();
    let mut tasks = Vec::new();
    for s in list.into_iter().filter(|s| s.url.starts_with("http")) {
        let http = state.http_insecure.clone();
        tasks.push(tokio::spawn(async move {
            let up = http.get(&s.url).timeout(Duration::from_secs(5)).send().await.is_ok();
            json!({"name": s.name, "url": s.url, "up": up})
        }));
    }
    let mut out = Vec::new();
    for t in tasks {
        if let Ok(v) = t.await {
            out.push(v);
        }
    }
    Ok(out)
}

// ---------------- custom actions ----------------
#[tauri::command]
async fn custom_list() -> Value {
    custom_list_now()
}

fn custom_list_now() -> Value {
    let path = custom::default_path();
    match custom::load(&path) {
        Ok(f) => {
            let names = custom::secret_names(&f);
            json!({"path": path, "exists": path.exists(), "actions": custom::summary(&f),
                "secrets": names.iter().map(|n| json!({"name": n, "set": secrets::has(&format!("custom:{n}"))})).collect::<Vec<_>>()})
        }
        Err(e) => json!({"path": path, "exists": true, "actions": [], "secrets": [], "error": e}),
    }
}

fn custom_action(index: usize) -> Result<custom::Action, String> {
    let f = custom::load(&custom::default_path())?;
    let mut a = f.actions.get(index).cloned().ok_or("that action is gone; reload the file")?;
    // secrets of custom actions are stored as "custom:NAME"
    let fix = |s: &str| s.replace("{{secret:", "{{secret:custom:");
    a.url = fix(&a.url);
    a.body = fix(&a.body);
    for v in a.headers.values_mut() {
        *v = fix(v);
    }
    if let Some(w) = a.watch.as_mut() {
        w.url = fix(&w.url);
    }
    Ok(a)
}

#[tauri::command]
async fn custom_run(app: AppHandle, state: tauri::State<'_, Arc<AppState>>, index: usize, input: Option<String>, file: Option<String>) -> Result<Value, String> {
    let a = custom_action(index)?;
    let r = custom::run(&state.http, &a, input.as_deref().unwrap_or("").trim(), file.as_deref()).await?;
    if let Some(c) = r["copy"].as_str().filter(|c| !c.is_empty()) {
        let _ = app.clipboard().write_text(c.to_string());
    }
    Ok(r)
}

#[tauri::command]
async fn custom_watch(state: tauri::State<'_, Arc<AppState>>, index: usize, ids: Vec<String>) -> Result<Value, String> {
    let a = custom_action(index)?;
    custom::watch(&state.http, &a, &ids).await
}

#[tauri::command]
async fn custom_import(path: String) -> Result<Value, String> {
    let p = PathBuf::from(&path);
    custom::load(&p)?;
    let dst = custom::default_path();
    if dst.exists() {
        let _ = std::fs::copy(&dst, dst.with_extension("json.bak"));
    }
    std::fs::copy(&p, &dst).map_err(|e| e.to_string())?;
    Ok(custom_list_now())
}

/// Write the example custom actions file if there's none yet.
#[tauri::command]
fn custom_example() -> Result<(), String> {
    let p = custom::default_path();
    if p.exists() {
        return Err("You already have a custom actions file.".into());
    }
    std::fs::write(&p, include_str!("../../examples/custom-actions.example.json")).map_err(|e| e.to_string())
}

#[tauri::command]
fn reveal_data() {
    let _ = Command::new("open").arg(data_dir()).spawn();
}

// ---------------- models + chat ----------------
#[tauri::command]
fn models_info(state: tauri::State<Arc<AppState>>) -> Value {
    let s = state.settings.lock().clone();
    json!({
        "installed": models::installed(&s.model_files),
        "recommended": models::recommended(),
        "current": s.model,
        "status": state.engine.status.lock().unwrap().clone(),
        "downloading": state.downloading.lock().clone(),
        "folder": models::dir(),
    })
}

#[tauri::command]
async fn model_download(app: AppHandle, state: tauri::State<'_, Arc<AppState>>, id: String) -> Result<String, String> {
    {
        let mut d = state.downloading.lock();
        if d.is_some() {
            return Err("Another model is downloading. Wait for it or cancel it first.".into());
        }
        *d = Some(id.clone());
    }
    state.dl_cancel.store(false, Ordering::SeqCst);
    let app2 = app.clone();
    let id2 = id.clone();
    let r = models::download(&state.http, &id, &state.dl_cancel, move |done, total| {
        let _ = app2.emit("model-progress", json!({"id": id2, "done": done, "total": total}));
    })
    .await;
    *state.downloading.lock() = None;
    let _ = app.emit("model-progress", json!({"id": id, "finished": true, "error": r.as_ref().err()}));
    let p = r?;
    let path = p.to_string_lossy().to_string();
    // first model: start using it straight away
    let empty = state.settings.lock().model.is_empty() || !PathBuf::from(&state.settings.lock().model).exists();
    if empty {
        let pp = path.clone();
        update(&app, move |s| s.model = pp);
    }
    Ok(path)
}

#[tauri::command]
fn model_cancel(state: tauri::State<Arc<AppState>>) {
    state.dl_cancel.store(true, Ordering::SeqCst);
}

#[tauri::command]
fn model_add(app: AppHandle, path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    let mut head = [0u8; 4];
    let ok = std::fs::File::open(&p).and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head)).is_ok() && &head == b"GGUF";
    if !ok {
        return Err("That isn't a GGUF model file.".into());
    }
    update(&app, |s| {
        if !s.model_files.contains(&path) {
            s.model_files.push(path.clone());
        }
        s.model = path;
    });
    Ok(())
}

/// Forget an added file, or move one of Clippy's own downloads to the Bin.
#[tauri::command]
async fn model_remove(app: AppHandle, state: tauri::State<'_, Arc<AppState>>, path: String) -> Result<(), String> {
    let p = PathBuf::from(&path);
    if state.settings.lock().model == path {
        state.engine.unload();
    }
    if p.parent() == Some(models::dir().as_path()) {
        let bin = dirs::home_dir().ok_or("no home folder")?.join(".Trash").join(p.file_name().ok_or("bad path")?);
        std::fs::rename(&p, &bin).map_err(|e| format!("Couldn't move it to the Bin: {e}"))?;
    }
    update(&app, |s| {
        s.model_files.retain(|f| f != &path);
        if s.model == path {
            s.model = String::new();
        }
    });
    Ok(())
}

#[tauri::command]
async fn model_unload(state: tauri::State<'_, Arc<AppState>>) -> Result<(), String> {
    state.engine.unload();
    Ok(())
}

#[tauri::command]
fn set_context(state: tauri::State<Arc<AppState>>, text: String) {
    *state.context.lock() = text.chars().take(3000).collect();
}

const FACTS: &str = "Mac facts to rely on: screenshot Cmd+Shift+3 (whole screen), Cmd+Shift+4 (drag an area), Cmd+Shift+4 then Space (a window), Cmd+Shift+5 (all tools, screen recording). \
Force quit Cmd+Option+Esc (the Mac's Ctrl+Alt+Delete). Spotlight Cmd+Space. Switch apps Cmd+Tab, windows of one app Cmd+`. Close window Cmd+W, quit app Cmd+Q, hide Cmd+H, minimise Cmd+M. \
Copy/cut/paste/undo Cmd+C/X/V/Z, redo Cmd+Shift+Z. Lock screen Control+Cmd+Q. Emoji Control+Cmd+Space. Delete a file Cmd+Delete; rename in Finder with Return. \
Move a copied file in Finder Cmd+Option+V. Hidden files in Finder Cmd+Shift+. (period). Delete forward Fn+Delete. Line start/end Cmd+Left/Right. Activity Monitor is the Task Manager. \
Full screen Control+Cmd+F. Go to folder Cmd+Shift+G. App settings Cmd+, (comma).";

const SARCASTIC: &str = "Personality: you are sarcastic, cheeky and funny, like a grumpy coworker who helps anyway. Tease the user lightly, roll your eyes in words, \
and you may swear a little (damn, hell, crap, bloody, the odd shit). Never slurs, never insults about who someone is, never actually mean. \
The real answer must still be correct and useful.";

pub(crate) fn system_prompt(s: &Settings, context: &str, extra: &str) -> String {
    let date = Command::new("date").arg("+%A %-d %B %Y, %H:%M").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    let mut p = format!(
        "You are Clippy, a paperclip assistant who lives on the user's Mac desktop. Your model runs on this Mac. \
         Keep answers short and clear: usually one to four sentences, more only when the user asks for detail or steps. \
         Only state facts about the user's Mac, homelab or accounts that appear in the status below; never make up numbers or results, \
         and say so if you can't see something. For general knowledge, answer from what you know or from a Wikipedia extract below; \
         if you aren't sure, say so instead of guessing. Use light Markdown (bold, lists, `code`) only when it helps. \
         When a keyboard shortcut helps, give the Mac one.\n{FACTS}\nIt is {date}."
    );
    if s.personality == "sarcastic" {
        p.push('\n');
        p.push_str(SARCASTIC);
    } else {
        p.push_str("\nPersonality: warm, upbeat and a little playful.");
    }
    if !context.trim().is_empty() {
        p.push_str("\nWhat Clippy can see right now (from the user's own Mac and home services; may be a few minutes old). Use it when asked, don't recite it unprompted:\n");
        p.push_str(context.trim());
    }
    if !extra.trim().is_empty() {
        p.push('\n');
        p.push_str(extra.trim());
    }
    p
}

#[tauri::command]
async fn chat_send(state: tauri::State<'_, Arc<AppState>>, messages: Vec<Msg>, on_event: Channel<Value>) -> Result<(), String> {
    let st: Arc<AppState> = state.inner().clone();
    let s = st.settings.lock().clone();
    if s.model.is_empty() || !PathBuf::from(&s.model).exists() {
        return Err("no-model".into());
    }
    if st.engine.status.lock().unwrap().busy {
        return Err("Still answering. Press Stop first.".into());
    }
    let msgs: Vec<Msg> = messages.into_iter().filter(|m| !m.content.trim().is_empty()).collect();
    let Some(question) = msgs.iter().rev().find(|m| m.role == "user").map(|m| m.content.clone()) else {
        return Err("Type something first.".into());
    };
    // live Home Assistant state and, for general questions, a Wikipedia lookup
    let ev = on_event.clone();
    let prep = assist::prepare(&st, &question, |t| {
        let _ = ev.send(json!({"kind": "status", "text": t}));
    })
    .await;
    if let Some((title, url)) = &prep.source {
        let _ = on_event.send(json!({"kind": "source", "title": title, "url": url}));
    }
    let req = GenReq {
        key: Key { path: PathBuf::from(&s.model), n_ctx: s.ctx },
        system: system_prompt(&s, &st.context.lock(), &prep.context),
        messages: msgs,
        temperature: if s.personality == "sarcastic" { 0.85 } else { 0.7 },
        max_tokens: 1500,
    };
    let st2 = st.clone();
    let already = prep.done.clone();
    // the model's own switch commands only count when the message really asked for an on/off
    let asked = assist::light_command(&question).is_some();
    let sink: engine::Sink = Box::new(move |ev| match ev {
        Ev::Piece(p) => {
            let _ = on_event.send(json!({"kind": "piece", "text": p}));
        }
        Ev::Done { text, tps, tokens, stopped, trimmed } => {
            let (mut clean, mut acts, blocked) = assist::take_actions(&text);
            if blocked {
                clean = format!("{clean}\n\n{}", assist::BLOCKED).trim().to_string();
            }
            if !asked {
                acts.clear();
            }
            if let Some(r) = &already {
                acts.clear(); // Clippy already switched them for this message
                // the model sometimes copies the result lines; keep only Clippy's own copy, added below
                let mine: Vec<&str> = r.lines().map(str::trim).collect();
                let kept: Vec<&str> = clean.lines().filter(|l| !mine.contains(&l.trim()) && !l.trim_start().starts_with('⚠')).collect();
                clean = format!("{}\n\n{r}", kept.join("\n").trim()).trim().to_string();
            }
            let done = move |text: String| json!({"kind": "done", "text": text, "tps": tps, "tokens": tokens, "stopped": stopped, "trimmed": trimmed});
            if acts.is_empty() || stopped {
                let _ = on_event.send(done(clean));
                return;
            }
            let (st, ch) = (st2.clone(), on_event.clone());
            let _ = ch.send(json!({"kind": "status", "text": "Switching the lights…"}));
            tauri::async_runtime::spawn(async move {
                let result = assist::run_actions(&st, &acts).await;
                let text = if clean.is_empty() { result } else { format!("{clean}\n\n{result}") };
                let _ = ch.send(done(text));
            });
        }
        Ev::Error(e) => {
            let _ = on_event.send(json!({"kind": "error", "message": e}));
        }
    });
    st.engine.generate(req, sink);
    Ok(())
}

#[tauri::command]
fn take_want_settings(state: tauri::State<Arc<AppState>>) -> bool {
    state.want_settings.swap(false, Ordering::Relaxed)
}

/// Test hook: CLIPPY_TEST_ASK="question" makes the chat window send it once when it opens.
#[tauri::command]
fn test_ask() -> Option<String> {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::SeqCst) {
        return None;
    }
    std::env::var("CLIPPY_TEST_ASK").ok().filter(|q| !q.is_empty())
}

#[tauri::command]
fn chat_stop(state: tauri::State<Arc<AppState>>) {
    state.engine.stop();
}

// ---------------- Mac ----------------
#[tauri::command]
async fn mac_health() -> Result<Value, String> {
    let out = tokio::process::Command::new(helper_path()).arg("health").output().await.map_err(|e| e.to_string())?;
    serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())
}

#[tauri::command]
fn request_key_access() -> String {
    Command::new(helper_path()).arg("keys-request").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

#[tauri::command]
fn copy_text(app: AppHandle, text: String) -> Result<(), String> {
    app.clipboard().write_text(text).map_err(|e| e.to_string())
}

/// Read the clipboard only when the user clicks "Paste" in a link box. Nothing watches it.
#[tauri::command]
fn paste_text(app: AppHandle) -> String {
    app.clipboard().read_text().unwrap_or_default().chars().take(2000).collect()
}

#[tauri::command]
fn open_url(url: String) -> Result<(), String> {
    if !(url.starts_with("http://") || url.starts_with("https://") || url.starts_with("x-apple.systempreferences:")) {
        return Err("not allowed".into());
    }
    Command::new("open").arg(url).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// The Swift helper's event stream (shortcut tips, idle time) → "helper" events for the buddy.
fn start_helper(app: AppHandle) {
    std::thread::spawn(move || loop {
        let child = Command::new(helper_path()).arg("watch").stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
        let Ok(mut child) = child else {
            std::thread::sleep(Duration::from_secs(30));
            continue;
        };
        let state = app.state::<Arc<AppState>>();
        *state.helper_in.lock() = child.stdin.take();
        if let Some(out) = child.stdout.take() {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    let _ = app.emit_to("buddy", "helper", v);
                }
            }
        }
        let _ = child.wait();
        std::thread::sleep(Duration::from_secs(5));
    });
}

/// Where the mouse is, relative to the buddy window, so Clippy's eyes can follow it.
fn start_eyes(app: AppHandle) {
    std::thread::spawn(move || {
        let mut last = (0.0, 0.0);
        loop {
            std::thread::sleep(Duration::from_millis(90));
            let Some(w) = app.get_webview_window("buddy") else { continue };
            if !w.is_visible().unwrap_or(false) {
                continue;
            }
            let (Ok(c), Ok(p), Ok(sf)) = (app.cursor_position(), w.outer_position(), w.scale_factor()) else { continue };
            let rel = ((c.x - p.x as f64) / sf, (c.y - p.y as f64) / sf);
            if (rel.0 - last.0).abs() + (rel.1 - last.1).abs() > 2.0 {
                last = rel;
                let _ = app.emit_to("buddy", "cursor", json!({"x": rel.0, "y": rel.1}));
            }
        }
    });
}

fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let s = app.state::<Arc<AppState>>().settings.lock().clone();
    let buddy = CheckMenuItem::with_id(app, "buddy", "Show Clippy", true, s.buddy, None::<&str>)?;
    let chat = MenuItem::with_id(app, "chat", "Chat with Clippy…", true, None::<&str>)?;
    let deck = MenuItem::with_id(app, "deck", "Open the Deck", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, Some("Cmd+,"))?;
    let quit = MenuItem::with_id(app, "quit", "Quit Clippy", true, Some("Cmd+Q"))?;
    let sep = || PredefinedMenuItem::separator(app);
    let menu = Menu::with_items(app, &[&chat, &deck, &buddy, &sep()?, &settings, &sep()?, &quit])?;
    let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;
    let tray = TrayIconBuilder::with_id("clippy")
        .icon(icon)
        .icon_as_template(true)
        .tooltip("Clippy")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, e| match e.id().as_ref() {
            "buddy" => update(app, |s| s.buddy = !s.buddy),
            "chat" => show_window(app.clone(), "chat".into()),
            "deck" => show_window(app.clone(), "deck".into()),
            "settings" => show_window(app.clone(), "settings".into()),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    let state = app.state::<Arc<AppState>>();
    *state.menu.lock() = Some(MenuRefs { buddy });
    *state.tray.lock() = Some(tray);
    Ok(())
}

fn make_state(notify: engine::Notify) -> Arc<AppState> {
    Arc::new(AppState {
        settings: Mutex::new(load_settings()),
        helper_in: Mutex::new(None),
        http: reqwest::Client::builder().connect_timeout(Duration::from_secs(5)).cookie_store(true).build().expect("http client"),
        http_insecure: reqwest::Client::builder().connect_timeout(Duration::from_secs(5)).cookie_store(true).danger_accept_invalid_certs(true).build().expect("http client"),
        menu: Mutex::new(None),
        tray: Mutex::new(None),
        engine: engine::Engine::start(notify),
        context: Mutex::new(String::new()),
        dl_cancel: Arc::new(AtomicBool::new(false)),
        downloading: Mutex::new(None),
        pihole_sid: Mutex::new(None),
        geo: Mutex::new(None),
        deck_open: AtomicBool::new(false),
        deck_h: Mutex::new(560.0),
        want_settings: AtomicBool::new(false),
    })
}

/// State for the command-line test mode (no windows; engine events are dropped).
pub(crate) fn new_state() -> Arc<AppState> {
    make_state(Arc::new(|_| {}))
}

pub fn run() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|a| a.starts_with("--")).unwrap_or(false) {
        std::process::exit(cli::main(&args[1..]));
    }
    let handle: Arc<Mutex<Option<AppHandle>>> = Arc::new(Mutex::new(None));
    let h2 = handle.clone();
    let notify: engine::Notify = Arc::new(move |v: Value| {
        if let Some(a) = h2.lock().as_ref() {
            let _ = a.emit("engine", v);
        }
    });
    let state = make_state(notify);
    write_settings(&state.settings.lock());
    tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            get_settings, save_settings, buddy_fit, buddy_moved, deck_collapse, deck_toggle, deck_fit, show_window, hide_window, set_tray_title, quit,
            integ, secrets_status, secret_set, check_services,
            custom_list, custom_run, custom_watch, custom_import, custom_example, reveal_data,
            models_info, model_download, model_cancel, model_add, model_remove, model_unload, set_context, chat_send, chat_stop, test_ask, take_want_settings,
            mac_health, request_key_access, copy_text, paste_text, open_url
        ])
        .setup(move |app| {
            *handle.lock() = Some(app.handle().clone());
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            build_tray(app.handle())?;
            let st = app.state::<Arc<AppState>>();
            apply_settings(app.handle(), &st);
            start_helper(app.handle().clone());
            start_eyes(app.handle().clone());
            // test hook: CLIPPY_OPEN=chat,settings,deck opens those windows at start
            if let Ok(list) = std::env::var("CLIPPY_OPEN") {
                for w in list.split(',') {
                    show_window(app.handle().clone(), w.trim().to_string());
                }
            }
            Ok(())
        })
        .on_window_event(|w, e| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = e {
                if w.label() == "chat" {
                    api.prevent_close();
                    let _ = w.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building Clippy")
        .run(|app, e| {
            if let tauri::RunEvent::Exit = e {
                let st = app.state::<Arc<AppState>>();
                // free the model first: llama.cpp's Metal backend asserts if it is still loaded at exit
                st.engine.unload();
                let _ = st.helper_in.lock().take().map(|mut s| s.flush());
            }
        });
}
