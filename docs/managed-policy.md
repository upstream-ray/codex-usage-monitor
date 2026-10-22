# Managed monitoring policy

Codex Usage can display an enabled provider's usage without generating model requests for authentication recovery. Passive monitoring is enabled by default. For centrally managed installations, administrators can also force passive monitoring and prohibit software updates.

These controls govern this executable's behavior. They do not reduce the permissions of the OAuth bearer tokens it reads, provide a credential sandbox, or protect against replacement of the executable by an administrator or another process with write access.

## User preferences

The user preferences live in `%APPDATA%\CodexUsage\settings.json`. The app resolves the roaming settings directory through Windows' folder API and loads preferences before credential discovery or update-helper actions.

| Preference | Normal default | Managed installation | Effect |
| --- | --- | --- | --- |
| `passive_monitoring` | `true` | `true` | Prevents CLI model requests for authentication recovery and the Claude Messages API fallback. |
| `updates_enabled` | `true` | `false` | Allows or blocks release checks, downloads, and application updates. |

Both fields are JSON booleans. Existing settings without these fields use the normal defaults. Unreadable or malformed preferences select passive monitoring with updates disabled. The settings menu saves explicit changes and applies them at the next relevant operation. External edits to user preferences should be made while the monitor is closed; update helpers separately reload persisted preferences at each permission check.

Provider selections are independent of these two preferences. Only enabled providers have their credentials discovered or usage polled. Claude Code remains selectable without a current CLI login, and the monitor preserves a choice to disable every provider. An authentication failure does not silently change that choice.

## Administrator-enforced policy

Deploy the [example policy](examples/managed-policy.json) as UTF-8 JSON without a BOM at:

```text
%PROGRAMDATA%\CodexUsage\policy.json
```

The app resolves this location from Windows' `FOLDERID_ProgramData` system folder, rather than trusting a `PROGRAMDATA` value supplied by the launching process. The usual location is `C:\ProgramData\CodexUsage\policy.json`.

```json
{
  "force_passive_monitoring": true,
  "disable_updates": true
}
```

The file accepts only these two fields, both strict JSON booleans. Missing fields default to `false`. A missing file imposes no machine restriction. An unreadable file, invalid JSON, unknown field, duplicate field, or wrong value type forces both restrictions on. The menu reports a policy-read error when it cannot accept the policy. To avoid transient parse failures, deploy changes with an atomic replacement of a complete file.

| Policy | Effective behavior |
| --- | --- |
| `force_passive_monitoring: true` | Passive monitoring stays on even if the user's preference is `false`. |
| `disable_updates: true` | Updates stay off even if the user's preference is `true`. |
| Both `false`, or no policy file | The user's preferences apply. |
| Policy cannot be read or validated | Passive monitoring stays on and updates stay off. |

Policy can only add restrictions. Enforced menu items show **Managed by your organization** and cannot be loosened through the application. Policy is rechecked at relevant process, network, and update boundaries, so an administrator's change applies to subsequent operations without restarting the monitor. It cannot retract a request already sent or terminate an external command that has already begun.

Use your organization's deployment tool to create and protect both the directory and the policy file. Standard users should have read access, with modification and deletion limited to the administrator or deployment identity. The per-user installer does not elevate, create this machine policy, or set its access-control list. A writable policy path does not provide an administrative control.

## Authentication behavior

Before deployment, check [Codex authentication compatibility](../README.md#account-support). Codex monitoring requires a supported Windows-readable `auth.json` and access to the fixed ChatGPT usage endpoint. It does not integrate with Codex keyring-only or ephemeral sessions, API-key authentication, browser-only logins, automatic WSL discovery, or custom gateways. Keep organizational storage and routing requirements in place; a managed installation does not remove these compatibility limits.

With passive monitoring enabled, Codex and Claude Code authentication recovery does not run `codex exec`, `claude -p`, or another model prompt. Claude's Messages API is not called merely to collect rate-limit headers. Existing usage endpoints can still be queried with the enabled provider's existing credentials. WSL discovery, when needed for an enabled Claude Code provider, reads credentials; passive mode does not prohibit all local processes.

Missing, expired, or rejected credentials produce an actionable sign-in message. The affected provider stops automatic retries until credentials change or the user explicitly selects **Refresh**. Wake and network-recovery events do not turn a blocked login into a new authentication attempt. The official Codex app or CLI handles Codex session renewal; after it updates the supported credential file or the user signs in again, select **Refresh**. Disabled providers remain untouched.

If the user explicitly disables passive monitoring and machine policy permits it, legacy recovery may run `codex exec .` or `claude -p .`, or submit a Claude Messages API request. These operations can perform inference, consume quota, incur charges, and invoke CLI hooks or project configuration. They are unsuitable when the organization permits usage monitoring but has not approved those operations.

## Update behavior

Normal installations with updates allowed automatically check GitHub approximately once per day and can check or install manually. Disabling updates blocks those requests and both the portable updater and legacy WinGet update path. The installed version remains visible.

The permission check applies inside backend functions, before downloading, before launching an updater, and before replacing the executable. The staged `--apply-update` helper reloads persisted preferences and machine policy after waiting for the parent process. The WinGet child likewise invokes the app's `--check-update-policy` mode and requires a successful exit before upgrading. Direct invocation of helper modes does not bypass the restrictions. Missing or malformed helper settings fail closed.

An update already handed off to an external installer cannot be canceled by changing the preference afterward. Use software-management controls and protect the approved executable when policy must constrain other processes too.

## Deploy an approved build

1. Review the source commit and follow [release verification](release-verification.md), or review the exact Windows CI artifact and workflow run for a preview build.
2. Retain the approved executable hash and package. Keep the installer, uninstall helper, sidecar hashes, and build metadata together.
3. Deploy the protected machine policy if restrictions must be mandatory.
4. Run the local managed installer with the approved executable hash:

```powershell
.\install.ps1 -Managed -SourcePath .\codex-usage.exe -ExpectedSha256 '<approved SHA256>'
```

`-Managed` makes no release lookup or download. It writes `passive_monitoring: true` and `updates_enabled: false` before the first launch, preserves unrelated settings, and verifies the executable and uninstall helper. Add `-NoLaunch` if the deployment process should control startup. An approved replacement is installed the same way; the disabled in-app updater is not needed.

For portable deployment, merge those two user preferences before first launch and deploy the same machine policy when mandatory. See [installation details](installation.md) for paths, startup behavior, and uninstall options.

## Validation

The Windows CI workflow runs the locked Rust tests and focused installer tests. Regression tests cover disabled providers, blocked authentication retries, absence of passive-mode inference fallbacks, managed policy parsing, disabled updater network calls, and staged-update rollback when policy changes. They exercise injected readers, request functions, and process boundaries without requiring real provider credentials. They do not replace the organization's account approval or review of the exact executable it deploys.
