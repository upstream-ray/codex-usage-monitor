# Local MCP server

Codex Usage can serve its current quota readings to AI agents through the
[Model Context Protocol](https://modelcontextprotocol.io). With it, Claude Code, Codex
CLI or another MCP client can ask how much quota is left before starting a long task,
or pick the provider with more headroom, using what the widget already shows.

The server is **off by default**. Turn it on with **Settings > Local MCP server for AI
agents (read-only)** in the widget or tray menu. The choice is saved in `settings.json`.

## Why tool calls cost no quota

A tool call returns a copy of the widget's last reading. It does not send a request to
Anthropic, OpenAI or Google, does not read credentials and does not run a model, so
agents can call it as often as they like. The widget keeps its normal refresh schedule
whether or not the server is on, and tool calls never change that schedule.

## Endpoint

| | |
| --- | --- |
| Address | `http://127.0.0.1:46827/mcp` |
| Transport | MCP Streamable HTTP: JSON-RPC 2.0 over `POST`, with JSON responses |
| Protocol versions | `2025-11-25`, `2025-06-18`, `2025-03-26` |
| Authentication | None. Only processes on this computer can connect |

To use another port, close the app and add `"mcp_port": 47000` (any port from 1024 up)
to `%APPDATA%\CodexUsage\settings.json`. If the port is in use or reserved by Windows,
the menu item says **port … is unavailable** and nothing is listening. Free the port or
choose another, then turn the item off and on again.

## Tools

| Tool | Returns |
| --- | --- |
| `get_claude_usage` | Claude Code quota |
| `get_codex_usage` | Codex quota |

Both tools take no arguments and are marked read-only, non-destructive, idempotent and
closed-world. The result is returned as `structuredContent`, with the same JSON as text:

```json
{
  "provider": "codex",
  "status": "ok",
  "five_hour": { "used_percent": 12.5, "remaining_percent": 87.5, "resets_at": 1800003600 },
  "weekly": { "used_percent": 61.0, "remaining_percent": 39.0, "resets_at": 1800360000 },
  "expired_windows": [],
  "lowest_remaining_percent": 39.0,
  "limiting_window": "weekly",
  "updated_at": 1799999940,
  "age_seconds": 60,
  "refresh_interval_seconds": 300,
  "data_source": "last_successful_poll",
  "error_category": null,
  "authentication_required": false
}
```

Every result has the same keys.

- `status`:
  - `ok`: a recent successful reading.
  - `stale`: the latest refresh failed, or the reading is older than the provider's
    refresh interval plus a grace period (the same rule as the widget's `*` marker). The
    values are from the last successful reading.
  - `unavailable`: no successful reading since the app started. `error_category` says
    why, if a refresh has failed.
  - `disabled`: the provider is not selected under **Models**. No values are reported.
- `five_hour` and `weekly`: `null` when there is no value. Percentages are not rounded.
  `resets_at` is a Unix time in seconds, or `null` when the provider gave none.
- `expired_windows`: windows whose reset time has passed since the reading. Their old
  value is withheld (reported as `null`) until the widget refreshes.
- `lowest_remaining_percent` and `limiting_window`: the window with the least quota left.
- `updated_at` and `age_seconds`: when the reading was taken, as Unix seconds, and its age.
- `refresh_interval_seconds`: the provider's current refresh interval.
- `error_category`: the latest refresh error, for example `rate_limited`,
  `network_unavailable` or `token_expired`. `authentication_required` is `true` when the
  provider needs you to sign in again.

## Set up a client

Turn the server on in the menu first. These commands change only the client's own
configuration.

**Claude Code**

```powershell
claude mcp add --transport http --scope user codex-usage http://127.0.0.1:46827/mcp
```

**Codex CLI**

```powershell
codex mcp add codex-usage --url http://127.0.0.1:46827/mcp
```

or add this to `~/.codex/config.toml`:

```toml
[mcp_servers.codex-usage]
url = "http://127.0.0.1:46827/mcp"
```

Other clients that support Streamable HTTP can use the same address. To check the
server by hand:

```powershell
Invoke-RestMethod -Method Post -Uri http://127.0.0.1:46827/mcp `
  -ContentType 'application/json' -Headers @{ Accept = 'application/json, text/event-stream' } `
  -Body '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_codex_usage"}}'
```

## Security model

- The server listens on `127.0.0.1` only and is never reachable from the network.
- It offers exactly the two tools above. There is no file access, command execution, URL
  fetching, prompt, resource or way to change settings.
- Results contain percentages, times and status only: no tokens, credentials, account
  names, email addresses or file paths.
- It accepts only `POST` requests to `/mcp` with a `Host` of `127.0.0.1:<port>` or
  `localhost:<port>`, and rejects any `Origin` other than those. This blocks web pages
  from reaching it through DNS rebinding or cross-site requests.
- Requests are limited to 16 KB of headers and 64 KB of body, chunked bodies are refused,
  and a request must arrive within 3 seconds.
- The monitor never edits Claude Code or Codex configuration. You add the server to a
  client yourself.

Any program running as you on this computer can read the same quota numbers the widget
shows. Leave the server off if that matters to you.

## Limitations

- The tools report Claude Code and Codex only. Antigravity is not exposed.
- Readings are only as fresh as the widget's refresh interval. A tool call cannot force a
  refresh. Use **Refresh** in the menu for that.
- Results are in memory only. After the app starts, tools return `unavailable` until the
  first successful refresh.
- Server-to-client streams (`GET`) and sessions are not offered. They are not needed for
  two request/response tools.
