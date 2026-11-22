//! What the chat gets on top of the model's own knowledge, fetched when the question is sent:
//!  - Home Assistant, live: lights, climate, temperature/humidity and other sensors, doors and windows.
//!    The model may switch lights (and only lights) by ending its answer with an `ACTION:` line,
//!    which is checked against the lights Home Assistant reported and then run here.
//!  - A Wikipedia lookup for "who is / what is / tell me about…" questions (Settings → Chat), since a
//!    small local model doesn't know everyone.

use crate::integrations;
use crate::AppState;
use serde_json::{json, Value};
use std::time::Duration;

pub struct Prepared {
    /// Extra text for the system prompt.
    pub context: String,
    /// Wikipedia page used: (title, url).
    pub source: Option<(String, String)>,
    /// What Clippy itself switched for this message, checked against Home Assistant afterwards.
    pub done: Option<String>,
}

fn num(v: &Value) -> Option<String> {
    match v {
        Value::Number(n) => n.as_f64().map(|f| if f.fract() == 0.0 { format!("{f:.0}") } else { format!("{f:.1}") }),
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// Home Assistant's live state as a few plain lines for the model.
pub fn ha_text(v: &Value) -> String {
    let mut out = vec!["Home Assistant (live, just read; answer only what is asked, in a sentence, and never paste these lines):".to_string()];
    let lights: Vec<String> = v["lights"].as_array().cloned().unwrap_or_default().iter().map(|l| {
        let st = if l["available"] == false { "unavailable".to_string() } else if l["on"] == true {
            l["brightness"].as_f64().map(|b| format!("on {b:.0}%")).unwrap_or_else(|| "on".into())
        } else { "off".into() };
        format!("{} [{}] {}", l["name"].as_str().unwrap_or(""), l["entity"].as_str().unwrap_or(""), st)
    }).collect();
    if !lights.is_empty() {
        out.push(format!("Lights: {}.", lights.join("; ")));
    }
    for c in v["climate"].as_array().cloned().unwrap_or_default() {
        let mut parts = vec![c["mode"].as_str().unwrap_or("").to_string()];
        if let Some(a) = c["action"].as_str() { parts.push(format!("({a})")); }
        if let Some(x) = num(&c["current"]) { parts.push(format!("{x}° now")); }
        if let Some(x) = num(&c["target"]) { parts.push(format!("set to {x}°")); }
        if let Some(x) = num(&c["humidity"]) { parts.push(format!("humidity {x}%")); }
        out.push(format!("Climate {}: {}.", c["name"].as_str().unwrap_or(""), parts.join(" ")));
    }
    for w in v["weather"].as_array().cloned().unwrap_or_default() {
        out.push(format!("Weather ({}): {}, {}{}, humidity {}%.", w["name"].as_str().unwrap_or(""), w["state"].as_str().unwrap_or(""),
            num(&w["temperature"]).unwrap_or_default(), w["unit"].as_str().unwrap_or("°"), num(&w["humidity"]).unwrap_or("?".into())));
    }
    let sensors: Vec<String> = v["sensors"].as_array().cloned().unwrap_or_default().iter()
        .map(|s| format!("{} {}{}", s["name"].as_str().unwrap_or(""), s["value"].as_str().unwrap_or(""), s["unit"].as_str().map(|u| if u == "%" { u.to_string() } else { format!(" {u}") }).unwrap_or_default()))
        .collect();
    if !sensors.is_empty() {
        out.push(format!("Sensors: {}.", sensors.join("; ")));
    }
    let openings: Vec<String> = v["openings"].as_array().cloned().unwrap_or_default().iter().map(|o| {
        let st = if o.get("locked").is_some() { if o["locked"] == true { "locked" } else { "unlocked" } } else if o["open"] == true { "open" } else { "closed" };
        format!("{} {st}", o["name"].as_str().unwrap_or(""))
    }).collect();
    if !openings.is_empty() {
        out.push(format!("Doors, windows and locks: {}.", openings.join("; ")));
    }
    let sw: Vec<String> = v["switches"].as_array().cloned().unwrap_or_default().iter()
        .map(|s| format!("{} {}", s["name"].as_str().unwrap_or(""), if s["on"] == true { "on" } else { "off" })).collect();
    if !sw.is_empty() {
        out.push(format!("Switches and fans (you can only read these, never switch them): {}.", sw.join("; ")));
    }
    if !lights.is_empty() {
        out.push("To switch lights when the user asks you to, write your short reply and then, as the very last line, \
            `ACTION: light on <entity ids>` or `ACTION: light off <entity ids>` using only ids in [brackets] from the Lights list \
            (several ids separated by spaces). Without that line nothing happens. Clippy runs it and adds the real result under \
            your reply, so never say a light is now on or off yourself, and never write ✓ lines. \
            Never try to switch anything that isn't in the Lights list.".to_string());
    }
    out.join("\n")
}

/// Status lines the model sometimes pastes back word for word; they're dropped from answers.
const ECHO: [&str; 7] = ["Lights:", "Sensors:", "Climate ", "Weather (", "Doors, windows and locks:", "Switches and fans", "Home Assistant (live"];

/// Light switches the model asked for: (on?, entity ids). Returns the answer without the ACTION lines
/// (and without pasted status lines), plus whether it tried to switch something that isn't a light.
pub fn take_actions(text: &str) -> (String, Vec<(bool, Vec<String>)>, bool) {
    let mut acts = vec![];
    let mut keep = vec![];
    let mut blocked = false;
    for line in text.lines() {
        let l = line.trim().trim_matches('`').trim();
        if ECHO.iter().any(|e| l.starts_with(e)) {
            continue;
        }
        let lower = l.to_lowercase();
        if (l.starts_with('✓') || l.starts_with('✔') || l.starts_with("✅")) && ["turned", "switched", "now on", "now off", "light", "lamp"].iter().any(|w| lower.contains(w)) {
            continue; // only Clippy writes the result line, after it has really switched something
        }
        let rest = l.strip_prefix("ACTION:").or_else(|| l.strip_prefix("Action:")).map(str::trim);
        match rest.and_then(|r| r.strip_prefix("light ")) {
            Some(r) => {
                let mut words = r.split_whitespace();
                let on = match words.next() {
                    Some("on") => true,
                    Some("off") => false,
                    _ => continue,
                };
                let all: Vec<String> = words.map(|w| w.trim_matches(|c: char| "[],.`".contains(c)).to_string()).collect();
                let ids: Vec<String> = all.iter().filter(|w| w.starts_with("light.")).cloned().collect();
                blocked |= ids.len() < all.len();
                if !ids.is_empty() {
                    acts.push((on, ids));
                }
            }
            None if rest.is_some() => blocked = true, // some other ACTION: drop it, never run it
            None => keep.push(line),
        }
    }
    (keep.join("\n").trim().to_string(), acts, blocked)
}

pub const BLOCKED: &str = "(I can only switch lights, so I left that alone.)";

/// Runs the light switches, only for lights Home Assistant reported, and says what happened.
pub async fn run_actions(st: &AppState, acts: &[(bool, Vec<String>)]) -> String {
    let known = match integrations::call(st, "home_assistant", "lights", &json!({})).await {
        Ok(v) => v["lights"].as_array().cloned().unwrap_or_default(),
        Err(e) => return format!("Couldn't reach Home Assistant to switch the lights: {e}"),
    };
    let name = |id: &str| known.iter().find(|l| l["entity"] == id).and_then(|l| l["name"].as_str()).unwrap_or(id).to_string();
    let mut lines = vec![];
    for (on, ids) in acts {
        let ok: Vec<String> = ids.iter().filter(|id| known.iter().any(|l| l["entity"] == id.as_str())).cloned().collect();
        if ok.is_empty() {
            lines.push("I couldn't find those lights in Home Assistant.".to_string());
            continue;
        }
        let names: Vec<String> = ok.iter().map(|i| name(i)).collect();
        match integrations::call(st, "home_assistant", "set", &json!({"on": on, "entities": ok})).await {
            Ok(_) => lines.push(format!("✓ Turned {} {}.", if *on { "on" } else { "off" }, names.join(", "))),
            Err(e) => lines.push(format!("Couldn't switch {}: {e}", names.join(", "))),
        }
    }
    lines.join("\n")
}


/// "turn off the desk lamp", "lights on", "switch on all the lights": which lights to switch, worked out here
/// rather than trusting a small model to emit the command. Returns (on?, entity ids). Questions never match.
fn words(q: &str) -> String {
    let low: String = q.to_lowercase().chars().map(|c| if c.is_alphanumeric() || c == '\'' { c } else { ' ' }).collect();
    format!(" {} ", low.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Is this message an on/off command (not a question)? Some(true) = on, Some(false) = off.
pub fn light_command(q: &str) -> Option<bool> {
    let low = words(q);
    if q.contains('?') || ["is ", "are ", "was ", "were ", "did ", "do ", "does ", "why ", "how ", "what ", "which ", "when ", "can you tell"].iter().any(|w| low.trim_start().starts_with(w)) {
        return None;
    }
    let off = low.contains(" off ") || low.contains(" lights out ") || low.contains(" kill the light");
    let on = low.contains(" on ");
    if on == off {
        return None;
    }
    let verb = ["turn ", "urn ", "switch ", "put ", "set "].iter().any(|v| low.contains(&format!(" {v}")))
        || ["light on", "lights on", "lamp on", "lamps on", "light off", "lights off", "lamp off", "lamps off", "lights out", "kill the light"].iter().any(|p| low.contains(p));
    verb.then_some(on)
}

pub fn light_intent(q: &str, lights: &[Value]) -> Option<(bool, Vec<String>)> {
    let on = light_command(q)?;
    let low = words(q);
    const SKIP: [&str; 8] = ["light", "lights", "lamp", "lamps", "the", "bulb", "bulbs", "strip"];
    let avail: Vec<&Value> = lights.iter().filter(|l| l["available"] != false).collect();
    let named: Vec<String> = avail.iter().filter(|l| {
        let name = l["name"].as_str().unwrap_or("").to_lowercase();
        let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() >= 3 && !SKIP.contains(w)).collect();
        !words.is_empty() && words.iter().all(|w| low.contains(&format!(" {w} ")) || low.contains(&format!(" {w}s ")))
    }).filter_map(|l| l["entity"].as_str().map(String::from)).collect();
    if !named.is_empty() {
        return Some((on, named));
    }
    let general = [" lights ", " all ", " everything ", " every light ", " the light ", " light ", " lamp ", " lamps "].iter().any(|w| low.contains(w));
    if general && !avail.is_empty() {
        return Some((on, avail.iter().filter_map(|l| l["entity"].as_str().map(String::from)).collect()));
    }
    None
}

/// Switches lights, then reads them back after a moment and says what really changed.
pub async fn switch_lights(st: &AppState, on: bool, ids: &[String], before: &[Value]) -> String {
    let name = |id: &str, from: &[Value]| from.iter().find(|l| l["entity"] == id).and_then(|l| l["name"].as_str()).unwrap_or(id).to_string();
    if let Err(e) = integrations::call(st, "home_assistant", "set", &json!({"on": on, "entities": ids})).await {
        return format!("⚠ Home Assistant didn't switch the lights: {e}");
    }
    tokio::time::sleep(Duration::from_millis(2000)).await;
    let after = match integrations::call(st, "home_assistant", "lights", &json!({})).await {
        Ok(v) => v["lights"].as_array().cloned().unwrap_or_default(),
        Err(_) => return format!("✓ Asked Home Assistant to turn {} {}. (Couldn't read them back to check.)", if on { "on" } else { "off" }, ids.iter().map(|i| name(i, before)).collect::<Vec<_>>().join(", ")),
    };
    let (mut ok, mut stuck) = (vec![], vec![]);
    for id in ids {
        let now_on = after.iter().find(|l| l["entity"] == id.as_str()).map(|l| l["on"] == true);
        if now_on == Some(on) { ok.push(name(id, before)) } else { stuck.push(name(id, before)) }
    }
    let word = if on { "on" } else { "off" };
    let mut out = vec![];
    if !ok.is_empty() {
        out.push(format!("✓ Turned {word} {}.", ok.join(", ")));
    }
    if !stuck.is_empty() {
        out.push(format!("⚠ Home Assistant took the command, but {} still {} {}: {} not responding (offline, or the bulb is switched off at the wall).",
            stuck.join(", "), if stuck.len() == 1 { "shows" } else { "show" }, if on { "off" } else { "on" }, if stuck.len() == 1 { "it may be" } else { "they may be" }));
    }
    out.join("\n")
}

/// Is this a general-knowledge question worth a Wikipedia lookup? Returns what to search for.
pub fn lookup_query(q: &str) -> Option<String> {
    let low = format!(" {} ", q.to_lowercase().replace(['?', '!', '"', '“', '”'], " "));
    const PERSONAL: [&str; 34] = [" my ", " mine ", " i ", " me ", " you ", " your ", "clippy", " mac", "homelab", "light", "lamp", "temperature",
        "humidity", "weather", " ac ", " fan ", " door", "window", "room", "download", "torrent", "pi-hole", "pihole", "proxmox", "docker",
        "container", "stock", "screenshot", "shortcut", "jellyfin", "radarr", "sonarr", " home ", "sensor"];
    let explicit = ["look up ", "lookup ", "search for ", "search ", "google ", "wikipedia ", "wiki "];
    let lead = ["who is ", "who was ", "who are ", "who were ", "what is ", "what was ", "what are ", "what's ", "whats ", "where is ",
        "where was ", "when was ", "when did ", "when is ", "tell me about ", "tell me something about ", "do you know ", "explain ",
        "define ", "describe ", "history of ", "how old is ", "how tall is ", "who invented ", "who founded ", "who wrote ", "who directed "];
    let t = low.trim_start();
    let (is_explicit, rest) = match explicit.iter().find(|p| t.starts_with(*p)) {
        Some(p) => (true, &t[p.len()..]),
        None => match lead.iter().find(|p| t.starts_with(*p)) {
            Some(p) => (false, &t[p.len()..]),
            None => return None,
        },
    };
    let padded = format!(" {} ", rest.trim());
    if !is_explicit && PERSONAL.iter().any(|w| padded.contains(w)) {
        return None;
    }
    let mut r = rest.trim().trim_start_matches("about ").trim_start_matches("the ").trim().to_string();
    for tail in [" please", " for me", " in short", " briefly"] {
        if let Some(x) = r.strip_suffix(tail) {
            r = x.trim().to_string();
        }
    }
    // keep the original capitalisation of what's left
    let orig = q.trim().trim_end_matches(['?', '!', '.']);
    if let Some(i) = orig.to_lowercase().find(&r) {
        if let Some(s) = orig.get(i..i + r.len()) {
            r = s.to_string();
        }
    }
    if r.chars().filter(|c| c.is_alphabetic()).count() < 3 || r.len() > 120 {
        return None;
    }
    Some(r)
}

/// Wikipedia's lead section for the best match: (title, extract, url).
pub async fn wikipedia(http: &reqwest::Client, q: &str) -> Option<(String, String, String)> {
    const UA: &str = "ClippyForMac/1.1 (desktop assistant)";
    let search = http.get("https://en.wikipedia.org/w/rest.php/v1/search/page").query(&[("q", q), ("limit", "3")])
        .header("User-Agent", UA).timeout(Duration::from_secs(5)).send().await.ok()?.json::<Value>().await.ok()?;
    for p in search["pages"].as_array().cloned().unwrap_or_default() {
        let key = p["key"].as_str()?.to_string();
        let url = format!("https://en.wikipedia.org/api/rest_v1/page/summary/{}", urlencoding::encode(&key));
        let Ok(r) = http.get(url).header("User-Agent", UA).timeout(Duration::from_secs(5)).send().await else { continue };
        let Ok(s) = r.json::<Value>().await else { continue };
        if s["type"] == "disambiguation" {
            continue;
        }
        let extract = s["extract"].as_str().unwrap_or("").trim().to_string();
        if extract.len() < 40 {
            continue;
        }
        let title = s["title"].as_str().unwrap_or(&key).to_string();
        let page = s["content_urls"]["desktop"]["page"].as_str().map(String::from).unwrap_or_else(|| format!("https://en.wikipedia.org/wiki/{key}"));
        let extract: String = extract.chars().take(1800).collect();
        return Some((title, extract, page));
    }
    None
}

/// Everything extra for this question. `status` reports what's being fetched ("Looking it up…").
pub async fn prepare(st: &AppState, question: &str, status: impl Fn(&str)) -> Prepared {
    let s = st.settings.lock().clone();
    let mut ctx = vec![];
    let mut source = None;
    let mut done = None;
    let ha_on = s.integrations.get("home_assistant").map(|i| i.on && !i.url.is_empty()).unwrap_or(false);
    let ha = async {
        if !ha_on {
            return None;
        }
        Some(integrations::call(st, "home_assistant", "states", &json!({})).await)
    };
    let q = if s.web_lookup { lookup_query(question) } else { None };
    if q.is_some() {
        status("Looking it up on Wikipedia…");
    }
    let wiki = async {
        match &q {
            Some(q) => wikipedia(&st.http, q).await,
            None => None,
        }
    };
    let (h, w) = tokio::join!(ha, wiki);
    match h {
        Some(Ok(mut v)) => {
            let lights = v["lights"].as_array().cloned().unwrap_or_default();
            if let Some((on, ids)) = light_intent(question, &lights) {
                status("Switching the lights…");
                let result = switch_lights(st, on, &ids, &lights).await;
                if let Ok(v2) = integrations::call(st, "home_assistant", "states", &json!({})).await {
                    v = v2;
                }
                ctx.push(format!("Clippy has ALREADY switched lights for this message. The real result, which is shown to the user under your reply \
                    (don't repeat it, don't write an ACTION line, just react in one short sentence): {result}"));
                done = Some(result);
            }
            ctx.push(ha_text(&v));
        }
        Some(Err(e)) => ctx.push(format!("Home Assistant: couldn't read it just now ({e}). Say so if the user asks about the home; never pretend.")),
        None => {}
    }
    if let Some((title, extract, url)) = w {
        ctx.push(format!("Wikipedia, “{title}” (looked up just now for this question; answer from it, in your own words):\n{extract}"));
        source = Some((title, url));
    } else if q.is_some() {
        ctx.push("A Wikipedia lookup for this question found nothing. If you don't know, say so; don't guess.".into());
    }
    Prepared { context: ctx.join("\n"), source, done }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queries() {
        assert_eq!(lookup_query("Who is Samantha Ruth Prabhu?").as_deref(), Some("Samantha Ruth Prabhu"));
        assert_eq!(lookup_query("tell me about the Eiffel Tower").as_deref(), Some("Eiffel Tower"));
        assert_eq!(lookup_query("look up my lights").as_deref(), Some("my lights"));
        assert_eq!(lookup_query("what is the temperature in the living room"), None);
        assert_eq!(lookup_query("who are you"), None);
        assert_eq!(lookup_query("turn off the lights"), None);
    }
    #[test]
    fn intents() {
        let l = vec![json!({"entity": "light.desk", "name": "Desk lamp", "available": true}), json!({"entity": "light.ceiling", "name": "Ceiling", "available": true}),
            json!({"entity": "light.broken", "name": "Broken bulb", "available": false})];
        let all = Some((true, vec!["light.desk".to_string(), "light.ceiling".to_string()]));
        assert_eq!(light_intent("turn on lights", &l), all);
        assert_eq!(light_intent("urn on all the lights", &l), all);
        assert_eq!(light_intent("Turn off the desk lamp", &l), Some((false, vec!["light.desk".to_string()])));
        assert_eq!(light_intent("lights off please", &l).map(|x| x.0), Some(false));
        assert_eq!(light_intent("are lights turned off", &l), None);
        assert_eq!(light_intent("is the desk lamp on?", &l), None);
        assert_eq!(light_intent("turn on the tv", &l), None);
        assert_eq!(light_command("are the lights on?"), None);
    }
    #[test]
    fn actions() {
        let (t, a, b) = take_actions("Sure, lights off.\nACTION: light off light.desk light.ceiling");
        assert_eq!(t, "Sure, lights off.");
        assert_eq!(a, vec![(false, vec!["light.desk".to_string(), "light.ceiling".to_string()])]);
        assert!(!b);
        let (_, a, b) = take_actions("`ACTION: switch off switch.ac`");
        assert!(a.is_empty() && b);
        let (t, _, _) = take_actions("It's 26 °C.\n\nSensors: Living room temperature 26.4 °C.");
        assert_eq!(t, "It's 26 °C.");
        let (t, _, _) = take_actions("Alright, lights on.\n\n✓ All lights now on.");
        assert_eq!(t, "Alright, lights on.");
    }
}
