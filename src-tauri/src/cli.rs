//! Command-line test mode (no windows), handy for checking a setup from Terminal:
//!   Clippy.app/Contents/MacOS/clippy --download qwen3-4b-2507
//!   Clippy.app/Contents/MacOS/clippy --chat <model.gguf> "question" [--rude]
//!   Clippy.app/Contents/MacOS/clippy --integ weather status '{"on":true,"extra":"London"}'
//!   Clippy.app/Contents/MacOS/clippy --custom <actions.json> <index> [input] [file]
//! Secrets come from CLIPPY_SECRET_<NAME> environment variables here, never the Keychain.
//! Don't run --chat while the Clippy app has a model loaded: two models may not fit in memory.

use crate::engine::{Ev, GenReq, Key, Msg};
use crate::{assist, custom, integrations, models, new_state, system_prompt};
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

pub fn main(args: &[String]) -> i32 {
    crate::secrets::TEST_MODE.store(true, std::sync::atomic::Ordering::Relaxed);
    let rt = tokio::runtime::Runtime::new().expect("runtime");
    let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("--download") => {
            let mut last = 0u64;
            let r = rt.block_on(models::download(&reqwest::Client::new(), &arg(1), &AtomicBool::new(false), |d, t| {
                let pct = d * 100 / t.max(1);
                if pct >= last + 5 {
                    last = pct;
                    eprintln!("{pct}% ({:.2} of {:.2} GB)", d as f64 / 1e9, t as f64 / 1e9);
                }
            }));
            match r {
                Ok(p) => {
                    println!("{}", p.display());
                    0
                }
                Err(e) => {
                    eprintln!("download failed: {e}");
                    1
                }
            }
        }
        Some("--chat") => {
            let st = new_state();
            // CLIPPY_HA_URL points the chat at a (mock) Home Assistant; its token is CLIPPY_SECRET_HOME_ASSISTANT
            if let Ok(u) = std::env::var("CLIPPY_HA_URL") {
                st.settings.lock().integrations.insert("home_assistant".into(), integrations::Integ { on: true, url: u, ..Default::default() });
            }
            let mut s = st.settings.lock().clone();
            s.personality = if args.iter().any(|a| a == "--rude") { "sarcastic".into() } else { "normal".into() };
            let prep = rt.block_on(assist::prepare(&st, &arg(2), |t| eprintln!("[{t}]")));
            if let Some((t, u)) = &prep.source {
                eprintln!("[source: {t} {u}]");
            }
            if let Some(d) = &prep.done {
                eprintln!("[switched before answering: {d}]");
            }
            let (tx, rx) = std::sync::mpsc::channel();
            let req = GenReq {
                key: Key { path: PathBuf::from(arg(1)), n_ctx: 4096 },
                system: system_prompt(&s, "", &prep.context),
                messages: vec![Msg { role: "user".into(), content: arg(2) }],
                temperature: if s.personality == "sarcastic" { 0.85 } else { 0.7 },
                max_tokens: 400,
            };
            let t0 = std::time::Instant::now();
            st.engine.generate(req, Box::new(move |ev| match ev {
                Ev::Piece(p) => {
                    print!("{p}");
                    let _ = std::io::stdout().flush();
                }
                Ev::Done { tps, tokens, text, .. } => {
                    let _ = tx.send(Ok((tps, tokens, text)));
                }
                Ev::Error(e) => {
                    let _ = tx.send(Err(e));
                }
            }));
            let r = rx.recv().unwrap_or(Err("engine stopped".into()));
            st.engine.unload();
            if let Ok((_, _, text)) = &r {
                let (clean, mut acts, blocked) = assist::take_actions(text);
                if prep.done.is_some() || assist::light_command(&arg(2)).is_none() {
                    acts.clear(); // same rule as the app
                }
                eprintln!("\n[shown in chat: {clean}{}]", if blocked { format!(" {}", assist::BLOCKED) } else { String::new() });
                if !acts.is_empty() {
                    eprintln!("\n[actions: {}]", rt.block_on(assist::run_actions(&st, &acts)));
                }
            }
            match r {
                Ok((tps, tokens, _)) => {
                    eprintln!("\n[{tokens} tokens, {tps:.1} tok/s, {:.1} s including load]", t0.elapsed().as_secs_f32());
                    0
                }
                Err(e) => {
                    eprintln!("\nerror: {e}");
                    1
                }
            }
        }
        Some("--integ") => {
            let st = new_state();
            if let Ok(cfg) = serde_json::from_str::<integrations::Integ>(&arg(3)) {
                st.settings.lock().integrations.insert(arg(1), cfg);
            }
            let r = rt.block_on(integrations::call(&st, &arg(1), &arg(2), &serde_json::from_str::<Value>(&arg(4)).unwrap_or(json!({}))));
            st.engine.unload();
            match r {
                Ok(v) => {
                    println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    1
                }
            }
        }
        Some("--custom") => {
            let f = match custom::load(&PathBuf::from(arg(1))) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("{e}");
                    return 1;
                }
            };
            let Some(a) = arg(2).parse::<usize>().ok().and_then(|i| f.actions.get(i).cloned()) else {
                eprintln!("no action {}", arg(2));
                return 1;
            };
            let file = args.get(4).cloned();
            match rt.block_on(custom::run(&reqwest::Client::new(), &a, &arg(3), file.as_deref())) {
                Ok(v) => {
                    println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                    let ids: Vec<String> = v["watch"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
                    if !ids.is_empty() {
                        if let Ok(w) = rt.block_on(custom::watch(&reqwest::Client::new(), &a, &ids)) {
                            println!("watch: {w}");
                        }
                    }
                    0
                }
                Err(e) => {
                    eprintln!("error: {e}");
                    1
                }
            }
        }
        _ => {
            eprintln!("usage: clippy --download <id> | --chat <model.gguf> <question> [--rude] | --integ <name> <action> [config] [args] | --custom <file> <index> [input] [file]");
            2
        }
    }
}
