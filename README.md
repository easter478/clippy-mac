# Clippy for Mac

A small paperclip-with-eyes that lives on your Mac desktop. It chats with you using a **Qwen model that runs entirely on your Mac**, keeps an eye on your homelab, and gives you a **Deck** of one-click tiles for the things you turn on.

It's an original character and isn't affiliated with Microsoft.

- **Chat on your Mac.** Pick a Qwen model you already have, or download a recommended one with one click. The model runs on your Mac.
- **Knows things.** Questions like "who is…" or "tell me about…" are looked up on Wikipedia first, so even a small model answers correctly. This can be turned off.
- **Talks to your home.** Ask about temperature, humidity, the AC or doors, or say "turn off the desk lamp". Home Assistant data is read live when you ask. Clippy itself switches the lights (it never relies on the model to send the command) and checks afterwards that each one really changed.
- **Two personalities.** *Nice*, or *Rude*: sarcastic, mildly sweary, never mean.
- **Homelab and home integrations.** Each one is optional: Home Assistant lights, Proxmox, Portainer, Pi-hole, Jellyfin, Radarr, Sonarr, qBittorrent, stocks, weather, plain "is it up?" checks, and a Lian Li screen.
- **Custom actions.** Add your own Deck tiles that call any HTTP service, using a JSON file. Secrets stay in the Keychain.
- **Stays quiet.** Each kind of message has its own on/off switch. A kind goes quiet after two "Not now"s, and Clippy naps while you're away.

## Install

1. Download `Clippy-<version>.dmg`, open it and drag **Clippy** into **Applications**.
2. The app isn't notarised by Apple, so the first time: right-click Clippy in Applications → **Open** (or System Settings → Privacy & Security → **Open Anyway**).
3. Clippy appears on the desktop and as a paperclip in the menu bar. Click him to chat. Settings (⚙, or ⌘,) and the Deck's tiles (the grid button) are both inside the chat window.

macOS may ask you for these:

| Prompt | Why | What breaks if you say no |
|---|---|---|
| Find devices on local networks | Talking to your homelab services | Integrations on your LAN can't connect |
| Keychain access ("Clippy wants to use…") | Reading the tokens you saved in Settings | Integrations that need a token |
| Input Monitoring (only if you click **Allow…** under *Windows → Mac tips*) | Spotting Windows shortcuts like Ctrl+C so it can suggest the Mac one | Only the tips; everything else works |

**Requirements:** a Mac with Apple silicon and macOS 14 or later. You need 8 GB of memory for the small models and 16 GB or more for the 8B model and up.

## Models

Clippy runs GGUF models with a built-in [llama.cpp](https://github.com/ggml-org/llama.cpp) engine, accelerated with Metal. These models are in Settings → Chat:

| Model | Download | Good for |
|---|---|---|
| Qwen3 1.7B (Q8) | 1.8 GB | 8 GB Macs; quick answers |
| Qwen3 4B Instruct 2507 (Q4_K_M) | 2.5 GB | The default for most Macs |
| Qwen3 8B (Q4_K_M) | 5.0 GB | 16 GB Macs |
| Qwen3 14B (Q4_K_M) | 9 GB | 24 GB Macs |
| Qwen3 30B-A3B Instruct 2507 (Q4_K_M) | 18.6 GB | 32 GB Macs and up |

- Clippy suggests the biggest model that suits your Mac's memory.
- Downloads come from Hugging Face. They can be paused and resumed, and are saved in `~/Library/Application Support/app.clippy.desktop/models`.
- **Add a model file…** uses any Qwen (or other ChatML) GGUF you already have, where it is. Nothing is copied.
- The model is unloaded from memory after a few idle minutes (you can change this).

## Integrations

Turn each one on in Settings → Integrations, then press **Test**. Tokens and passwords are stored in the macOS Keychain (service "Clippy"), never in a settings file.

| Integration | What Clippy does | What it needs |
|---|---|---|
| Home Assistant | Answers in chat with live lights, temperature, humidity, climate, doors/windows and switch states; switches **lights only** (from chat or the Lights tile), never switches or plugs | Address and a long-lived token (Profile → Security) |
| Proxmox VE | Warns when a VM or container stops; can start it again | Address, API token ID (`user@pam!name`) and secret |
| Portainer | Warns when a container stops or is unhealthy; can restart it | Address and an access token (My account → Access tokens) |
| Pi-hole v6 | Today's blocking stats; pauses blocking for 5 minutes | Address and password |
| Jellyfin | What's playing and what's new | Address and an API key |
| Radarr / Sonarr | Queue, what's coming up, recently imported | Address and an API key (Settings → General) |
| qBittorrent | What's downloading; a ping when something finishes | Address, user name and password (if you use one) |
| Stocks | Your watchlist, plus a ping on big moves (Yahoo Finance) | Symbols, e.g. `AAPL, MSFT, BTC-USD` |
| Weather | Today and tomorrow (Open-Meteo, no account) | A town name |
| Homelab checks | Simple "is this address answering?" checks | Name and address for each one |
| Lian Li screen (optional) | Shows Clippy's messages on a Lian Li LCD | A compatible screen server (not included) |

Self-signed HTTPS (common for Proxmox, Portainer and Pi-hole) works once you tick **Self-signed HTTPS certificate** for that integration.

Clippy reads your integrations into the chat as context, so you can ask "is my homelab OK?" and get a real answer. It's told never to make numbers up.

## Custom actions

Custom actions add your own Deck tiles for services only you have, like a file drop, a download service or a webhook. They live in a JSON file:

```
~/Library/Application Support/app.clippy.desktop/custom-actions.json
```

Settings → Custom actions has **Create an example**, **Import…**, **Reload** and **Show in Finder**. See [examples/custom-actions.example.json](examples/custom-actions.example.json) for a full example.

```json
{
  "actions": [
    {
      "name": "Notify my phone",
      "icon": "send",
      "input": "text",
      "prompt": "What should I send?",
      "method": "POST",
      "url": "https://ntfy.sh/{{secret:ntfy-topic}}",
      "body": "{{input}}",
      "result": "Sent."
    }
  ]
}
```

| Field | Meaning |
|---|---|
| `input` | `none`, `text`, `url` (has a Paste button) or `file` |
| `drop` | `true`: this action runs when a file is dropped on Clippy |
| `method`, `url`, `headers`, `body` | The HTTP request. A body of exactly `{{file}}` streams the file |
| `result`, `copy`, `open` | What Clippy says, what it copies to the clipboard, what it opens |
| `watch` | Polls a job list until a job is done or failed (see the example) |
| `icon` | `bolt`, `upload`, `download`, `link`, `play`, `send`, `home`, `folder`, `film`, `bulb`, `server`… |

Placeholders:

| Placeholder | Becomes |
|---|---|
| `{{input}}` | What the user typed or pasted |
| `{{file.name}}`, `{{file.size}}` | The chosen or dropped file |
| `{{secret:NAME}}` | A value you set in Settings → Custom actions (kept in the Keychain) |
| `{{response.a.b}}` | A field from the JSON answer |

Values are URL-encoded in `url` and JSON-escaped in JSON bodies.

## Build from source

You need: Xcode command line tools, [Rust](https://rustup.rs), the Tauri CLI (`cargo install tauri-cli`) and CMake (`brew install cmake`).

```bash
zsh tools/build.sh
```

This builds the Swift helper and the Tauri app, signs them and creates `dist/Clippy-<version>.dmg`.

- To sign with your own identity, set `CLIPPY_SIGN_ID` to a code-signing identity; without it the app is signed ad hoc.
- If a `personal/build.env` file exists, it's read first (it isn't tracked by Git).

### Project layout

| Path | What |
|---|---|
| `src-tauri/src/lib.rs` | App, windows, settings, tray, commands, the chat's system prompt |
| `src-tauri/src/engine.rs` | llama.cpp engine: Qwen chat template, streaming, KV-cache reuse, idle unload |
| `src-tauri/src/models.rs` | Recommended models, downloads (resumable), installed list |
| `src-tauri/src/integrations.rs` | All built-in integrations |
| `src-tauri/src/assist.rs` | What the chat gets per question: live Home Assistant state, light actions, Wikipedia lookup |
| `src-tauri/src/custom.rs` | Custom actions: templating, file streaming, job watching |
| `src-tauri/src/secrets.rs` | Keychain storage |
| `src-tauri/src/cli.rs` | Command-line test mode |
| `helper/main.swift` | Sidecar for idle time, Mac health and the shortcut tips |
| `ui/` | Buddy, Deck and chat windows (plain HTML/JS); Settings is a page inside the chat; `tiles.js` is the tile list the Deck and the chat share |
| `tools/mock-services.py` | Fake home services for testing integrations |

### Test from Terminal

The app binary has a no-window test mode. In this mode, secrets come from `CLIPPY_SECRET_<NAME>` environment variables, and the Keychain is never touched.

```bash
Clippy.app/Contents/MacOS/clippy --download qwen3-4b-2507
```

```bash
Clippy.app/Contents/MacOS/clippy --chat ~/path/model.gguf "Is a paperclip a tool?" --rude
```

```bash
Clippy.app/Contents/MacOS/clippy --integ weather status '{"on":true,"extra":"London"}'
```

```bash
CLIPPY_SECRET_NTFY_TOPIC=my-topic Clippy.app/Contents/MacOS/clippy --custom examples/custom-actions.example.json 0 "hello"
```

`python3 tools/mock-services.py` starts fake Home Assistant, Proxmox, Portainer, Pi-hole, Jellyfin, Radarr, Sonarr and qBittorrent services on port 8765.

## Privacy

- Chat runs on your Mac. The only internet requests are:
  - model downloads (Hugging Face);
  - Wikipedia searches for general questions ("who is…", "what is…"), only the search words, which you can turn off in Settings → Chat;
  - stocks (Yahoo Finance) and weather (Open-Meteo), and only if you turn them on;
  - whatever services you configure yourself.
- The chat history is stored only in the chat window's local storage on your Mac. **New chat** clears it.
- There's no clipboard watching, analytics or telemetry.
