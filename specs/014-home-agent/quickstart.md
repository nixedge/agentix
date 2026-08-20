# Quickstart: agentix-agent Home Assistant

## Prerequisites

- Nix flakes enabled
- agentix-daemon running (`nix run .#dev` then `agentix-daemon`)
- A Google Cloud project with Calendar API and Gmail API enabled
- ntfy app installed on your phone, subscribed to your chosen topic

## 1. Build

```bash
cd ~/home/agentix/014-home-agent
nix develop --command cargo build -p agentix-agent
```

## 2. Configure

Create the config file:

```bash
mkdir -p ~/.config/agentix-agent
cat > ~/.config/agentix-agent/agent.toml <<'EOF'
gateway_url = "http://localhost:11434"
local_model = "qwen3:8b"
db_path = "~/.local/share/agentix-agent/agent.db"
bind_addr = "0.0.0.0:8765"

[ntfy]
url = "https://ntfy.sh"
topic = "your-secret-topic-here"

[google]
credentials_path = "~/.config/agentix-agent/google-credentials.json"
token_cache_path  = "~/.config/agentix-agent/google-token.json"
calendar_id       = "primary"

[scheduler]
notification_interval_secs = 300
gmail_scan_interval_secs   = 900
event_reminder_lead_secs   = 1800
overdue_grace_secs         = 300
EOF
```

## 3. Set up Google OAuth2

1. Go to [Google Cloud Console](https://console.cloud.google.com) → APIs & Services → Credentials
2. Create an OAuth 2.0 Client ID → Desktop app
3. Download the JSON and save it to `~/.config/agentix-agent/google-credentials.json`

On first run, agentix-agent will print a device-flow URL and code:

```
Visit https://accounts.google.com/device?user_code=XXXX-XXXX
Enter code: XXXX-XXXX
Waiting for authorization...
```

Authorise in your browser once. The token is cached and auto-refreshed.

## 4. Run

```bash
nix develop --command agentix-agent
# or with a custom config path:
nix develop --command agentix-agent --config /path/to/agent.toml
```

The agent binds on port 8765 and starts background schedulers.

## 5. Open the Chat UI

On your phone (same LAN or VPN): `http://<server-ip>:8765/`

You'll see the chat interface. Type a message or tap the microphone button to send a voice note.

## 6. CLI Queries (no daemon required)

```bash
# List open todos
agentix-agent todos list

# Create a todo
agentix-agent todos add "Change the furnace filter" --due 2026-09-01

# Complete a todo
agentix-agent todos done <id>

# List today's calendar events
agentix-agent calendar today
```

## 7. Test push notifications

```bash
agentix-agent notify test --title "Test" --body "agentix-agent is working"
```

You should receive a notification in the ntfy app within a few seconds.

## Logs

```bash
RUST_LOG=agentix_agent=debug nix develop --command agentix-agent
```

## NixOS Service (optional)

Add to your NixOS configuration:

```nix
services.agentix-agent = {
  enable = true;
  configFile = "/etc/agentix-agent/agent.toml";
};
```

(The NixOS module is created as part of this feature's flake integration task.)
