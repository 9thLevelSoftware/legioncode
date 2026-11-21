###-begin-t3-completions-###
#
# Static completion script for PowerShell
#
# Installation:
#   t3 --completions powershell > t3-completion.ps1
#   then add this line to your $PROFILE (use the full path to the file):
#   . <PATH>\t3-completion.ps1
#

$global:_t3_Completions = [hashtable]::new([System.StringComparer]::Ordinal)
$global:_t3_Completions[''] = @{
  subcommands = @(
    @{ name = 'help'; description = 'Show command help.' }
    @{ name = 'serve'; description = 'Run T3 Code in this terminal, with no browser.' }
    @{ name = 'app'; description = 'Open a project in the running T3 Code desktop app.' }
    @{ name = 'pair'; description = 'Mint a pairing token for a running T3 Code server and print it as a QR code.' }
    @{ name = 'auth'; description = 'Manage the local auth control plane for headless deployments.' }
    @{ name = 'environment'; description = 'Link other T3 Code environments, so agents here can work there.' }
    @{ name = 'browser'; description = 'Manage T3''s headless browser on this host.' }
    @{ name = 'project'; description = 'Manage projects.' }
    @{ name = 'thread'; description = 'Start, message, and read threads on the running server.' }
    @{ name = 'up'; description = 'Set up remote access and the T3 Code background service.' }
    @{ name = 'status'; description = 'Show service setup and remote access settings.' }
    @{ name = 'service'; description = 'Manage the T3 Code background service.' }
    @{ name = 'update'; description = 'Download a newer t3 and switch this machine to it, including the background service when one is installed.' }
    @{ name = 'install'; description = 'Install this version as the `t3` command.' }
    @{ name = 'uninstall'; description = 'Remove t3 from this machine: the background service, the launcher, and every downloaded version. Your projects and threads are kept.' }
    @{ name = 'theme'; description = 'Inspect and set environment-wide theme defaults.' }
    @{ name = 'trace'; description = 'Inspect the local server trace file.' }
    @{ name = 'doctor'; description = 'Inspect this T3 Code data directory and repair it. Pauses and restarts the background service as needed.' }
    @{ name = 'policy'; description = 'Inspect the administrator''s managed policy.' }
    @{ name = 'triage'; description = 'Investigate a T3 Code problem on this machine with Claude Code, Codex, or Antigravity, and help file a good issue.' }
  )
  flags = @(
    @{ name = 'mode'; forms = @('--mode'); takesValue = $true; values = @('web', 'desktop'); pathType = $null; description = 'Runtime mode. `desktop` keeps loopback defaults unless overridden.' }
    @{ name = 'port'; forms = @('--port'); takesValue = $true; values = @(); pathType = $null; description = 'Port for the HTTP/WebSocket server.' }
    @{ name = 'host'; forms = @('--host'); takesValue = $true; values = @(); pathType = $null; description = 'Host/interface to bind (for example 127.0.0.1, 0.0.0.0, or a Tailnet IP).' }
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'dev-url'; forms = @('--dev-url'); takesValue = $true; values = @(); pathType = $null; description = 'Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).' }
    @{ name = 'no-browser'; forms = @('--no-browser'); takesValue = $false; negation = '--no-no-browser'; values = @(); pathType = $null; description = 'Disable automatic browser opening.' }
    @{ name = 'bootstrap-fd'; forms = @('--bootstrap-fd'); takesValue = $true; values = @(); pathType = $null; description = 'Read one-time bootstrap secrets from the given file descriptor.' }
    @{ name = 'auto-bootstrap-project-from-cwd'; forms = @('--auto-bootstrap-project-from-cwd'); takesValue = $false; negation = '--no-auto-bootstrap-project-from-cwd'; values = @(); pathType = $null; description = 'Create a project for the current working directory on startup when missing.' }
    @{ name = 'log-websocket-events'; forms = @('--log-websocket-events', '--log-ws-events'); takesValue = $false; negation = '--no-log-websocket-events'; values = @(); pathType = $null; description = 'Emit server-side logs for outbound WebSocket push traffic (equivalent to T3CODE_LOG_WS_EVENTS).' }
    @{ name = 'tailscale-serve'; forms = @('--tailscale-serve'); takesValue = $false; negation = '--no-tailscale-serve'; values = @(); pathType = $null; description = 'Configure Tailscale Serve to expose this backend over HTTPS on the Tailnet.' }
    @{ name = 'tailscale-serve-port'; forms = @('--tailscale-serve-port'); takesValue = $true; values = @(); pathType = $null; description = 'HTTPS port for Tailscale Serve when --tailscale-serve is enabled.' }
    @{ name = 'lan'; forms = @('--lan'); takesValue = $false; negation = '--no-lan'; values = @(); pathType = $null; description = 'Accept connections from your local network and tailnet, not only this machine (equivalent to T3CODE_LAN).' }
  )
  arguments = @(
    @{ name = 'cwd'; variadic = $false; values = @(); pathType = $null; description = 'Working directory for provider sessions (defaults to the current directory).' }
  )
}
$global:_t3_Completions['help'] = @{
  subcommands = @()
  flags = @()
  arguments = @()
}
$global:_t3_Completions['serve'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'mode'; forms = @('--mode'); takesValue = $true; values = @('web', 'desktop'); pathType = $null; description = 'Runtime mode. `desktop` keeps loopback defaults unless overridden.' }
    @{ name = 'port'; forms = @('--port'); takesValue = $true; values = @(); pathType = $null; description = 'Port for the HTTP/WebSocket server.' }
    @{ name = 'host'; forms = @('--host'); takesValue = $true; values = @(); pathType = $null; description = 'Host/interface to bind (for example 127.0.0.1, 0.0.0.0, or a Tailnet IP).' }
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'dev-url'; forms = @('--dev-url'); takesValue = $true; values = @(); pathType = $null; description = 'Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).' }
    @{ name = 'no-browser'; forms = @('--no-browser'); takesValue = $false; negation = '--no-no-browser'; values = @(); pathType = $null; description = 'Disable automatic browser opening.' }
    @{ name = 'bootstrap-fd'; forms = @('--bootstrap-fd'); takesValue = $true; values = @(); pathType = $null; description = 'Read one-time bootstrap secrets from the given file descriptor.' }
    @{ name = 'log-websocket-events'; forms = @('--log-websocket-events', '--log-ws-events'); takesValue = $false; negation = '--no-log-websocket-events'; values = @(); pathType = $null; description = 'Emit server-side logs for outbound WebSocket push traffic (equivalent to T3CODE_LOG_WS_EVENTS).' }
    @{ name = 'tailscale-serve'; forms = @('--tailscale-serve'); takesValue = $false; negation = '--no-tailscale-serve'; values = @(); pathType = $null; description = 'Configure Tailscale Serve to expose this backend over HTTPS on the Tailnet.' }
    @{ name = 'tailscale-serve-port'; forms = @('--tailscale-serve-port'); takesValue = $true; values = @(); pathType = $null; description = 'HTTPS port for Tailscale Serve when --tailscale-serve is enabled.' }
    @{ name = 'lan'; forms = @('--lan'); takesValue = $false; negation = '--no-lan'; values = @(); pathType = $null; description = 'Accept connections from your local network and tailnet, not only this machine (equivalent to T3CODE_LAN).' }
  )
  arguments = @(
    @{ name = 'cwd'; variadic = $false; values = @(); pathType = $null; description = 'Working directory for provider sessions (defaults to the current directory).' }
  )
}
$global:_t3_Completions['app'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
  )
  arguments = @(
    @{ name = 'path'; variadic = $false; values = @(); pathType = $null; description = 'Project directory. Default: current directory.' }
  )
}
$global:_t3_Completions['pair'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'scope'; forms = @('--scope'); takesValue = $true; repeatable = $true; values = @('orchestration:read', 'orchestration:operate', 'settings:write', 'providers:manage', 'environment:maintain', 'preview:operate', 'diagnostics:read', 'terminal:read', 'terminal:operate', 'filesystem:read', 'filesystem:write', 'source-control:write', 'access:read', 'access:write', 'relay:read', 'relay:write', 'relay:configure'); pathType = $null; description = 'Authorization scope to grant. Repeat for multiple scopes; replaces the default set: orchestration:read, orchestration:operate, settings:write, providers:manage, environment:maintain, preview:operate, diagnostics:read, terminal:read, terminal:operate, source-control:write, filesystem:read, filesystem:write, relay:read, access:read, access:write, relay:write.' }
    @{ name = 'ttl'; forms = @('--ttl'); takesValue = $true; values = @(); pathType = $null; description = 'Token TTL, for example `5m`, `1h`, or `15 minutes`. Defaults to 5 minutes.' }
    @{ name = 'label'; forms = @('--label'); takesValue = $true; values = @(); pathType = $null; description = 'Optional label shown in the server''s connections list.' }
    @{ name = 'tailscale'; forms = @('--tailscale'); takesValue = $false; negation = '--no-tailscale'; values = @(); pathType = $null; description = 'Publish the server over Tailscale Serve HTTPS and pair through the tailnet URL.' }
    @{ name = 'tailscale-serve-port'; forms = @('--tailscale-serve-port'); takesValue = $true; values = @(); pathType = $null; description = 'HTTPS port for Tailscale Serve when --tailscale is enabled.' }
  )
  arguments = @()
}
$global:_t3_Completions['auth'] = @{
  subcommands = @(
    @{ name = 'pairing'; description = 'Manage one-time client pairing tokens.' }
    @{ name = 'session'; description = 'Manage bearer sessions.' }
    @{ name = 'connect-device'; description = 'Approve and revoke devices that reach this host through T3 Connect.' }
  )
  flags = @()
  arguments = @()
}
$global:_t3_Completions['auth pairing'] = @{
  subcommands = @(
    @{ name = 'create'; description = 'Issue a new client pairing token.' }
    @{ name = 'list'; description = 'List active client pairing tokens without revealing their secrets.' }
    @{ name = 'revoke'; description = 'Revoke an active client pairing token.' }
  )
  flags = @()
  arguments = @()
}
$global:_t3_Completions['auth pairing create'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'dev-url'; forms = @('--dev-url'); takesValue = $true; values = @(); pathType = $null; description = 'Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).' }
    @{ name = 'scope'; forms = @('--scope'); takesValue = $true; repeatable = $true; values = @('orchestration:read', 'orchestration:operate', 'settings:write', 'providers:manage', 'environment:maintain', 'preview:operate', 'diagnostics:read', 'terminal:read', 'terminal:operate', 'filesystem:read', 'filesystem:write', 'source-control:write', 'access:read', 'access:write', 'relay:read', 'relay:write', 'relay:configure'); pathType = $null; description = 'Authorization scope to grant. Repeat for multiple scopes; replaces the default set: orchestration:read, orchestration:operate, settings:write, providers:manage, environment:maintain, preview:operate, diagnostics:read, terminal:read, terminal:operate, source-control:write, filesystem:read, filesystem:write, relay:read, access:read, access:write, relay:write.' }
    @{ name = 'ttl'; forms = @('--ttl'); takesValue = $true; values = @(); pathType = $null; description = 'TTL, for example `5m`, `1h`, `30d`, or `15 minutes`.' }
    @{ name = 'label'; forms = @('--label'); takesValue = $true; values = @(); pathType = $null; description = 'Optional human-readable label.' }
    @{ name = 'base-url'; forms = @('--base-url'); takesValue = $true; values = @(); pathType = $null; description = 'Optional public base URL used to print a ready `/pair#token=...` link.' }
    @{ name = 'json'; forms = @('--json'); takesValue = $false; negation = '--no-json'; values = @(); pathType = $null; description = 'Emit JSON instead of human-readable output.' }
  )
  arguments = @()
}
$global:_t3_Completions['auth pairing list'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'dev-url'; forms = @('--dev-url'); takesValue = $true; values = @(); pathType = $null; description = 'Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).' }
    @{ name = 'json'; forms = @('--json'); takesValue = $false; negation = '--no-json'; values = @(); pathType = $null; description = 'Emit JSON instead of human-readable output.' }
  )
  arguments = @()
}
$global:_t3_Completions['auth pairing revoke'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'dev-url'; forms = @('--dev-url'); takesValue = $true; values = @(); pathType = $null; description = 'Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).' }
  )
  arguments = @(
    @{ name = 'id'; variadic = $false; values = @(); pathType = $null; description = 'Pairing credential id to revoke.' }
  )
}
$global:_t3_Completions['auth session'] = @{
  subcommands = @(
    @{ name = 'issue'; description = 'Issue a scoped bearer access token for headless or remote clients.' }
    @{ name = 'list'; description = 'List active sessions without revealing bearer tokens.' }
    @{ name = 'revoke'; description = 'Revoke an active session. A T3 Connect device''s session takes the device''s trust and other sessions with it; if approval for new devices is off, the device can connect again on its own.' }
  )
  flags = @()
  arguments = @()
}
$global:_t3_Completions['auth session issue'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'dev-url'; forms = @('--dev-url'); takesValue = $true; values = @(); pathType = $null; description = 'Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).' }
    @{ name = 'scope'; forms = @('--scope'); takesValue = $true; repeatable = $true; values = @('orchestration:read', 'orchestration:operate', 'settings:write', 'providers:manage', 'environment:maintain', 'preview:operate', 'diagnostics:read', 'terminal:read', 'terminal:operate', 'filesystem:read', 'filesystem:write', 'source-control:write', 'access:read', 'access:write', 'relay:read', 'relay:write', 'relay:configure'); pathType = $null; description = 'Authorization scope to grant. Repeat for multiple scopes; replaces the default set: orchestration:read, orchestration:operate, settings:write, providers:manage, environment:maintain, preview:operate, diagnostics:read, terminal:read, terminal:operate, source-control:write, filesystem:read, filesystem:write, relay:read, access:read, access:write, relay:write.' }
    @{ name = 'ttl'; forms = @('--ttl'); takesValue = $true; values = @(); pathType = $null; description = 'TTL, for example `5m`, `1h`, `30d`, or `15 minutes`.' }
    @{ name = 'label'; forms = @('--label'); takesValue = $true; values = @(); pathType = $null; description = 'Optional human-readable label.' }
    @{ name = 'subject'; forms = @('--subject'); takesValue = $true; values = @(); pathType = $null; description = 'Optional session subject.' }
    @{ name = 'token-only'; forms = @('--token-only'); takesValue = $false; negation = '--no-token-only'; values = @(); pathType = $null; description = 'Print only the issued bearer token.' }
    @{ name = 'json'; forms = @('--json'); takesValue = $false; negation = '--no-json'; values = @(); pathType = $null; description = 'Emit JSON instead of human-readable output.' }
  )
  arguments = @()
}
$global:_t3_Completions['auth session list'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'dev-url'; forms = @('--dev-url'); takesValue = $true; values = @(); pathType = $null; description = 'Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).' }
    @{ name = 'json'; forms = @('--json'); takesValue = $false; negation = '--no-json'; values = @(); pathType = $null; description = 'Emit JSON instead of human-readable output.' }
  )
  arguments = @()
}
$global:_t3_Completions['auth session revoke'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'dev-url'; forms = @('--dev-url'); takesValue = $true; values = @(); pathType = $null; description = 'Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).' }
  )
  arguments = @(
    @{ name = 'session-id'; variadic = $false; values = @(); pathType = $null; description = 'Session id to revoke.' }
  )
}
$global:_t3_Completions['auth connect-device'] = @{
  subcommands = @(
    @{ name = 'list'; description = 'List T3 Connect devices waiting for approval and those already trusted.' }
    @{ name = 'approve'; description = 'Approve the waiting T3 Connect device that shows this code.' }
    @{ name = 'revoke'; description = 'Stop trusting a T3 Connect device and end its sessions.' }
  )
  flags = @()
  arguments = @()
}
$global:_t3_Completions['auth connect-device list'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'dev-url'; forms = @('--dev-url'); takesValue = $true; values = @(); pathType = $null; description = 'Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).' }
    @{ name = 'json'; forms = @('--json'); takesValue = $false; negation = '--no-json'; values = @(); pathType = $null; description = 'Emit JSON instead of human-readable output.' }
  )
  arguments = @()
}
$global:_t3_Completions['auth connect-device approve'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'dev-url'; forms = @('--dev-url'); takesValue = $true; values = @(); pathType = $null; description = 'Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).' }
  )
  arguments = @(
    @{ name = 'code'; variadic = $false; values = @(); pathType = $null; description = 'The code the device shows, such as `K7QM-2XFD`.' }
  )
}
$global:_t3_Completions['auth connect-device revoke'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'dev-url'; forms = @('--dev-url'); takesValue = $true; values = @(); pathType = $null; description = 'Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).' }
  )
  arguments = @(
    @{ name = 'code'; variadic = $false; values = @(); pathType = $null; description = 'The code the device shows, such as `K7QM-2XFD`.' }
  )
}
$global:_t3_Completions['environment'] = @{
  subcommands = @(
    @{ name = 'link'; description = ('Link another T3 Code environment, so agents here can work there. It shows up there under Authorized clients in Settings ' + [char]0x2192 + ' Sharing & access, where it can be revoked.') }
    @{ name = 'list'; description = 'List linked environments and whether each answers now.' }
    @{ name = 'unlink'; description = 'Forget a linked environment here. Revoke it in that environment''s Sharing & access to end its session there too.' }
  )
  flags = @()
  arguments = @()
}
$global:_t3_Completions['environment link'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'dev-url'; forms = @('--dev-url'); takesValue = $true; values = @(); pathType = $null; description = 'Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).' }
    @{ name = 'url'; forms = @('--url'); takesValue = $true; repeatable = $true; values = @(); pathType = $null; description = 'Another address for the same environment, tried after the first.' }
    @{ name = 'pairing-code'; forms = @('--pairing-code'); takesValue = $true; values = @(); pathType = $null; description = 'A pairing code from the other environment (`t3 pair` there).' }
    @{ name = 'access'; forms = @('--access'); takesValue = $true; values = @('read-only', 'approval-required', 'auto-accept-edits', 'auto', 'full-access'); pathType = $null; description = 'What this environment''s agents may do there: read-only, or the broadest runtime mode.' }
  )
  arguments = @(
    @{ name = 'url'; variadic = $false; values = @(); pathType = $null; description = 'Where the other environment answers: its T3 Connect, Tailscale, LAN or loopback address.' }
  )
}
$global:_t3_Completions['environment list'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'dev-url'; forms = @('--dev-url'); takesValue = $true; values = @(); pathType = $null; description = 'Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).' }
    @{ name = 'json'; forms = @('--json'); takesValue = $false; negation = '--no-json'; values = @(); pathType = $null; description = 'Emit JSON instead of human-readable output.' }
  )
  arguments = @()
}
$global:_t3_Completions['environment unlink'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'dev-url'; forms = @('--dev-url'); takesValue = $true; values = @(); pathType = $null; description = 'Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).' }
  )
  arguments = @(
    @{ name = 'environment-id'; variadic = $false; values = @(); pathType = $null; description = 'The linked environment''s id, from `t3 environment list`.' }
  )
}
$global:_t3_Completions['browser'] = @{
  subcommands = @(
    @{ name = 'setup'; description = 'Set up this Linux host for T3''s browser: allow Chrome''s sandbox and install its libraries and fonts.' }
  )
  flags = @()
  arguments = @()
}
$global:_t3_Completions['browser setup'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
  )
  arguments = @()
}
$global:_t3_Completions['project'] = @{
  subcommands = @(
    @{ name = 'add'; description = 'Add a project.' }
    @{ name = 'remove'; description = 'Remove a project.' }
    @{ name = 'rename'; description = 'Rename a project.' }
  )
  flags = @()
  arguments = @()
}
$global:_t3_Completions['project add'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'title'; forms = @('--title'); takesValue = $true; values = @(); pathType = $null; description = 'Optional project title.' }
  )
  arguments = @(
    @{ name = 'path'; variadic = $false; values = @(); pathType = $null; description = 'Workspace root to add as a project.' }
  )
}
$global:_t3_Completions['project remove'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'force'; forms = @('--force'); takesValue = $false; negation = '--no-force'; values = @(); pathType = $null; description = 'Delete the project and all of its threads.' }
  )
  arguments = @(
    @{ name = 'project'; variadic = $false; values = @(); pathType = $null; description = 'Project id or workspace root to remove.' }
  )
}
$global:_t3_Completions['project rename'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
  )
  arguments = @(
    @{ name = 'project'; variadic = $false; values = @(); pathType = $null; description = 'Project id or workspace root to rename.' }
    @{ name = 'title'; variadic = $false; values = @(); pathType = $null; description = 'New project title.' }
  )
}
$global:_t3_Completions['thread'] = @{
  subcommands = @(
    @{ name = 'list'; description = 'List a project''s threads, newest first.' }
    @{ name = 'start'; description = 'Start a thread in a project and print its id.' }
    @{ name = 'send'; description = 'Send a message to a thread. A busy thread gets it as its next turn; nothing is interrupted.' }
    @{ name = 'show'; description = 'Show a thread''s state and its latest messages.' }
  )
  flags = @()
  arguments = @()
}
$global:_t3_Completions['thread list'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'project'; forms = @('--project'); takesValue = $true; values = @(); pathType = $null; description = 'Project id or a path inside the project. Defaults to this directory.' }
    @{ name = 'json'; forms = @('--json'); takesValue = $false; negation = '--no-json'; values = @(); pathType = $null; description = 'Print JSON instead of text.' }
  )
  arguments = @()
}
$global:_t3_Completions['thread start'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'title'; forms = @('--title'); takesValue = $true; values = @(); pathType = $null; description = 'Thread title. Defaults to the message''s first line.' }
    @{ name = 'generate-title'; forms = @('--generate-title'); takesValue = $false; negation = '--no-generate-title'; values = @(); pathType = $null; description = 'Generate the thread''s title from the message. Until it''s ready, the thread shows --title or the first line.' }
    @{ name = 'provider'; forms = @('--provider'); takesValue = $true; values = @(); pathType = $null; description = 'Provider instance id. Pass with --model.' }
    @{ name = 'model'; forms = @('--model'); takesValue = $true; values = @(); pathType = $null; description = 'Model slug. Defaults to the project''s default model.' }
    @{ name = 'option'; forms = @('--option'); takesValue = $true; repeatable = $true; values = @(); pathType = $null; description = 'Model option as id=value, such as effort=high. Repeat for more. Needs --provider and --model.' }
    @{ name = 'worktree'; forms = @('--worktree'); takesValue = $true; values = @(); pathType = $null; description = 'Run in a new git worktree based on this branch, tag, or commit, instead of the project folder.' }
    @{ name = 'branch'; forms = @('--branch'); takesValue = $true; values = @(); pathType = $null; description = 'Branch name for the new worktree.' }
    @{ name = 'output-schema'; forms = @('--output-schema'); takesValue = $true; values = @(); pathType = $null; description = 'File holding a JSON Schema. The agent must return data matching it; --wait --json prints it as `output`.' }
    @{ name = 'wait'; forms = @('--wait'); takesValue = $false; negation = '--no-wait'; values = @(); pathType = $null; description = 'Wait for the agent to finish and print its reply. Exits 1 if the run fails and 2 if it waits for an answer.' }
    @{ name = 'json'; forms = @('--json'); takesValue = $false; negation = '--no-json'; values = @(); pathType = $null; description = 'Print JSON instead of text.' }
  )
  arguments = @(
    @{ name = 'project'; variadic = $false; values = @(); pathType = $null; description = 'Project id or a path inside the project, such as .' }
    @{ name = 'message'; variadic = $false; values = @(); pathType = $null; description = 'Message for the agent, or - to read it from stdin.' }
  )
}
$global:_t3_Completions['thread send'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'output-schema'; forms = @('--output-schema'); takesValue = $true; values = @(); pathType = $null; description = 'File holding a JSON Schema. The agent must return data matching it; --wait --json prints it as `output`.' }
    @{ name = 'wait'; forms = @('--wait'); takesValue = $false; negation = '--no-wait'; values = @(); pathType = $null; description = 'Wait for the agent to finish and print its reply. Exits 1 if the run fails and 2 if it waits for an answer.' }
    @{ name = 'json'; forms = @('--json'); takesValue = $false; negation = '--no-json'; values = @(); pathType = $null; description = 'Print JSON instead of text.' }
  )
  arguments = @(
    @{ name = 'thread'; variadic = $false; values = @(); pathType = $null; description = 'Thread id.' }
    @{ name = 'message'; variadic = $false; values = @(); pathType = $null; description = 'Message for the agent, or - to read it from stdin.' }
  )
}
$global:_t3_Completions['thread show'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'messages'; forms = @('--messages'); takesValue = $true; values = @(); pathType = $null; description = 'How many of the latest messages to print.' }
    @{ name = 'json'; forms = @('--json'); takesValue = $false; negation = '--no-json'; values = @(); pathType = $null; description = 'Print JSON instead of text.' }
  )
  arguments = @(
    @{ name = 'thread'; variadic = $false; values = @(); pathType = $null; description = 'Thread id.' }
  )
}
$global:_t3_Completions['up'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'connect'; forms = @('--connect'); takesValue = $false; negation = '--no-connect'; values = @(); pathType = $null; description = 'Allow access through T3 Connect with your T3 account. Turn off with --no-connect.' }
    @{ name = 'lan'; forms = @('--lan'); takesValue = $false; negation = '--no-lan'; values = @(); pathType = $null; description = 'Allow LAN and tailnet connections. Anyone there can reach the port; a pairing code is the only lock. Turn off with --no-lan.' }
    @{ name = 'yes'; forms = @('--yes'); takesValue = $false; negation = '--no-yes'; values = @(); pathType = $null; description = 'Ask nothing. Keep the access settings; a new machine gets T3 Connect. A restart stops running agent turns.' }
    @{ name = 'headless'; forms = @('--headless'); takesValue = $false; negation = '--no-headless'; values = @(); pathType = $null; description = 'Sign in with a URL and code on any device. Automatic without a terminal or over SSH.' }
  )
  arguments = @()
}
$global:_t3_Completions['status'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'json'; forms = @('--json'); takesValue = $false; negation = '--no-json'; values = @(); pathType = $null; description = 'Print the settings as JSON. Does not test connections.' }
  )
  arguments = @()
}
$global:_t3_Completions['service'] = @{
  subcommands = @(
    @{ name = 'install'; description = 'Install T3 Code as a background service for this user. `t3 up` also chooses how your other devices reach it.' }
    @{ name = 'restart'; description = 'Restart the background service. Picks up a version installed by `t3 update` that was not restarted at the time.' }
    @{ name = 'uninstall'; description = 'Stop and remove the T3 Code background service.' }
    @{ name = 'status'; description = 'Show whether the T3 Code background service is installed.' }
    @{ name = 'protect'; description = 'Protect the T3 Code server from agents under memory pressure (Linux with systemd). Asks for sudo.' }
  )
  flags = @()
  arguments = @()
}
$global:_t3_Completions['service install'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'allow-downgrade'; forms = @('--allow-downgrade'); takesValue = $false; negation = '--no-allow-downgrade'; values = @(); pathType = $null; description = 'Allow replacing a newer installed service with this older CLI version.' }
  )
  arguments = @()
}
$global:_t3_Completions['service restart'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
  )
  arguments = @()
}
$global:_t3_Completions['service uninstall'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
  )
  arguments = @()
}
$global:_t3_Completions['service status'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
  )
  arguments = @()
}
$global:_t3_Completions['service protect'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'undo'; forms = @('--undo'); takesValue = $false; negation = '--no-undo'; values = @(); pathType = $null; description = 'Remove the protection this command set up.' }
    @{ name = 'yes'; forms = @('--yes'); takesValue = $false; negation = '--no-yes'; values = @(); pathType = $null; description = 'Answer yes to every question, including SSH and Tailscale.' }
  )
  arguments = @()
}
$global:_t3_Completions['update'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'channel'; forms = @('--channel'); takesValue = $true; values = @('stable', 'nightly', 'preview'); pathType = $null; description = 'Release channel to follow. Defaults to the channel this t3 was published on.' }
    @{ name = 'allow-downgrade'; forms = @('--allow-downgrade'); takesValue = $false; negation = '--no-allow-downgrade'; values = @(); pathType = $null; description = 'Allow moving to an older version than the one running.' }
    @{ name = 'yes'; forms = @('--yes', '-y'); takesValue = $false; negation = '--no-yes'; values = @(); pathType = $null; description = 'Restart the background service without asking. Required to restart it from a script, where there is no prompt.' }
  )
  arguments = @(
    @{ name = 'version'; variadic = $false; values = @(); pathType = $null; description = 'Exact version to install. Defaults to the newest release on the channel.' }
  )
}
$global:_t3_Completions['install'] = @{
  subcommands = @()
  flags = @()
  arguments = @()
}
$global:_t3_Completions['uninstall'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'yes'; forms = @('--yes', '-y'); takesValue = $false; negation = '--no-yes'; values = @(); pathType = $null; description = 'Remove everything without asking. Required from a script, where there is no prompt.' }
  )
  arguments = @()
}
$global:_t3_Completions['theme'] = @{
  subcommands = @(
    @{ name = 'set'; description = 'Set the environment''s theme. The web app this server serves and the desktop app on this machine switch to it; a desktop app connected from another machine does not.' }
    @{ name = 'clear'; description = 'Remove the environment''s theme; clients keep what they have.' }
    @{ name = 'show'; description = 'Show the environment''s theme and its published themes.' }
  )
  flags = @()
  arguments = @()
}
$global:_t3_Completions['theme set'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'id'; forms = @('--id'); takesValue = $true; values = @(); pathType = $null; description = 'Theme id to publish a file under, instead of its filename.' }
  )
  arguments = @(
    @{ name = 'theme'; variadic = $false; values = @(); pathType = $null; description = 'A theme id (a built-in, or one this machine publishes, where themes/nightfall.json is "nightfall"), or a path to a theme JSON file to publish and set in one step.' }
  )
}
$global:_t3_Completions['theme clear'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
  )
  arguments = @()
}
$global:_t3_Completions['theme show'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
  )
  arguments = @()
}
$global:_t3_Completions['trace'] = @{
  subcommands = @(
    @{ name = 'summary'; description = 'Summarize the local server trace file: counts, rates, and latency.' }
  )
  flags = @()
  arguments = @()
}
$global:_t3_Completions['trace summary'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'since'; forms = @('--since'); takesValue = $true; values = @(); pathType = $null; description = 'Only count spans that ended in this window, for example 30m or 2h.' }
    @{ name = 'limit'; forms = @('--limit'); takesValue = $true; values = @(); pathType = $null; description = 'Number of span names to show, busiest first.' }
  )
  arguments = @()
}
$global:_t3_Completions['doctor'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'fix'; forms = @('--fix'); takesValue = $false; negation = '--no-fix'; values = @(); pathType = $null; description = 'Apply the recommended repairs without prompting.' }
    @{ name = 'dry-run'; forms = @('--dry-run'); takesValue = $false; negation = '--no-dry-run'; values = @(); pathType = $null; description = 'Show the repairs without changing anything, even with --fix.' }
  )
  arguments = @()
}
$global:_t3_Completions['policy'] = @{
  subcommands = @(
    @{ name = 'show'; description = 'Show the policy installed on this machine, and whether the running server loaded it.' }
    @{ name = 'check'; description = 'Validate a policy file before deploying it.' }
    @{ name = 'settings'; description = 'List the settings a policy can manage, with their defaults.' }
    @{ name = 'templates'; description = 'Write the Windows Group Policy template and a macOS configuration profile.' }
  )
  flags = @()
  arguments = @()
}
$global:_t3_Completions['policy show'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
  )
  arguments = @()
}
$global:_t3_Completions['policy check'] = @{
  subcommands = @()
  flags = @()
  arguments = @(
    @{ name = 'file'; variadic = $false; values = @(); pathType = $null; description = 'Policy file to validate.' }
  )
}
$global:_t3_Completions['policy settings'] = @{
  subcommands = @()
  flags = @()
  arguments = @()
}
$global:_t3_Completions['policy templates'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'policy'; forms = @('--policy'); takesValue = $true; values = @(); pathType = $null; description = 'Policy file to put in the macOS profile. Default: a small sample.' }
  )
  arguments = @(
    @{ name = 'directory'; variadic = $false; values = @(); pathType = $null; description = 'Directory to write the templates to.' }
  )
}
$global:_t3_Completions['triage'] = @{
  subcommands = @()
  flags = @(
    @{ name = 'base-dir'; forms = @('--base-dir'); takesValue = $true; values = @(); pathType = $null; description = 'Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).' }
    @{ name = 'agent'; forms = @('--agent'); takesValue = $true; values = @('claude', 'codex', 'antigravity'); pathType = $null; description = 'Agent CLI to use. Default: ask when multiple are installed.' }
    @{ name = 'model'; forms = @('--model'); takesValue = $true; values = @(); pathType = $null; description = 'Model passed through to the agent CLI. Default: the agent''s default.' }
  )
  arguments = @()
}

Register-ArgumentCompleter -Native -CommandName 't3' -ScriptBlock {
  param($wordToComplete, $commandAst, $cursorPosition)

  $spec = $global:_t3_Completions
  $results = [System.Collections.Generic.List[System.Management.Automation.CompletionResult]]::new()
  $booleanLiterals = @('true', 'yes', 'on', '1', 'y', 'false', 'no', 'off', '0', 'n')

  $index = {
    param($context)
    $lookup = [hashtable]::new([System.StringComparer]::Ordinal)
    foreach ($flag in $context.flags) {
      foreach ($form in $flag.forms) { $lookup[$form] = $flag }
    }
    $lookup
  }

  $add = {
    param([string]$value, [string]$type, $tooltip, [string]$typed, [string]$textPrefix)
    if (-not $value.StartsWith($typed, [System.StringComparison]::OrdinalIgnoreCase)) { return }
    $text = $value
    if ($value -notmatch '^[\w./:%+=\\-]+$') { $text = "'" + ($value -replace "['\u2018-\u201b]", '$0$0') + "'" }
    $text = $textPrefix + $text
    $label = $value
    if (-not $label) { $label = $text }
    if (-not $tooltip) { $tooltip = $label }
    $results.Add([System.Management.Automation.CompletionResult]::new($text, $label, $type, $tooltip))
  }

  $addValues = {
    param($entry, [string]$typed, [string]$textPrefix)
    foreach ($value in $entry.values) {
      if ($textPrefix -eq '' -and (& $optionLike $value)) { continue }
      & $add $value 'ParameterValue' $entry.description $typed $textPrefix
    }
    if ($entry.pathType) {
      foreach ($file in [System.Management.Automation.CompletionCompleters]::CompleteFilename($typed)) {
        if ($entry.pathType -eq 'directory' -and $file.ResultType -ne 'ProviderContainer') { continue }
        $results.Add([System.Management.Automation.CompletionResult]::new($textPrefix + $file.CompletionText, $file.ListItemText, $file.ResultType, $file.ToolTip))
      }
    }
  }

  $addFlags = {
    param([string]$typed)
    foreach ($flag in $context.flags) {
      if (-not $flag.repeatable -and $used.ContainsKey($flag.name)) { continue }
      foreach ($form in $flag.forms) {
        & $add $form 'ParameterName' $flag.description $typed ''
      }
      if ($flag.negation) {
        $tooltip = $null
        if ($flag.description) { $tooltip = 'Disable ' + $flag.name }
        & $add $flag.negation 'ParameterName' $tooltip $typed ''
      }
    }
  }

  # As in the CLI lexer, nothing after '--' is an option, and negative numbers
  # and a lone '-' are values.
  $optionLike = {
    param([string]$word)
    -not $endOfOptions -and $word -match '^-.' -and $word -notmatch '^-(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?$'
  }

  # A typed value that has no candidates completes to itself, so the engine
  # does not fall back to listing files.
  $keepTyped = {
    if ($results.Count -eq 0 -and $current) {
      $results.Add([System.Management.Automation.CompletionResult]::new($current, $current, 'ParameterValue', $current))
    }
  }

  $path = ''
  $context = $spec[$path]
  $lookup = & $index $context
  $used = [hashtable]::new([System.StringComparer]::Ordinal)
  $pending = $null
  $endOfOptions = $false
  $position = 0

  # The engine strips the quotes from a quoted word.
  $typed = [string]$wordToComplete
  if ($typed -match '^([''"])(.*?)\1?$') { $typed = $Matches[2] }
  $current = $null

  $elements = $commandAst.CommandElements
  for ($i = 1; $i -lt $elements.Count; $i++) {
    $element = $elements[$i]
    $word = $element.Extent.Text
    if ($element -is [System.Management.Automation.Language.StringConstantExpressionAst]) { $word = $element.Value }
    if ($element.Extent.EndOffset -ge $cursorPosition) {
      if ($element -is [System.Management.Automation.Language.ConstantExpressionAst] -and $element.Extent.StartOffset -le $cursorPosition) {
        $current = $element.Extent.Text
        if ($element.Extent.EndOffset -eq $cursorPosition) { $typed = $word }
      }
      break
    }

    # A flag takes the next word as its value unless it is an option or '--';
    # a boolean flag only takes a boolean literal.
    $isOption = & $optionLike $word
    if ($null -ne $pending) {
      $flag = $pending
      $pending = $null
      if ($flag.takesValue) {
        if (-not $isOption) { continue }
      } elseif ($booleanLiterals -ccontains $word) { continue }
    }
    if ($endOfOptions) { $position++; continue }
    if ($word -ceq '--') { $endOfOptions = $true; continue }

    if ($isOption) {
      $hasValue = $word.Contains('=')
      if ($hasValue) {
        $forms = @($word.Substring(0, $word.IndexOf('=')))
      } elseif ($word.StartsWith('--')) {
        $forms = @($word)
      } else {
        $forms = @(foreach ($char in $word.Substring(1).ToCharArray()) { '-' + $char })
      }
      foreach ($form in $forms) {
        $pending = $null
        $flag = $lookup[$form]
        # As in the CLI parser, a negation wins over an alias but not over a
        # canonical name.
        if ($null -eq $flag -or $form -cne ('--' + $flag.name)) {
          $negated = $null
          foreach ($candidate in $context.flags) {
            if ($candidate.negation -ceq $form) { $negated = $candidate }
          }
          if ($null -ne $negated) {
            $used[$negated.name] = $true
            continue
          }
        }
        if ($null -eq $flag) { continue }
        $used[$flag.name] = $true
        # A flag with an optional value only takes a value attached with '='.
        if (-not $hasValue -and -not $flag.optionalValue) { $pending = $flag }
      }
      continue
    }

    if ($position -eq 0 -and $context.subcommands.name -ccontains $word) {
      if ($path) { $path = $path + ' ' + $word } else { $path = $word }
      $context = $spec[$path]
      $lookup = & $index $context
      $used = [hashtable]::new([System.StringComparer]::Ordinal)
      continue
    }
    $position++
  }

  $typedIsOption = & $optionLike $typed
  if ($null -ne $pending -and $pending.takesValue -and -not $typedIsOption) {
    & $addValues $pending $typed ''
    & $keepTyped
    return $results
  }

  if ($typedIsOption) {
    if ($typed -match '^(-[^=]+)=(.*)$') {
      $form = $Matches[1]
      $flag = $lookup[$form]
      if ($null -ne $flag -and $flag.takesValue) {
        & $addValues $flag $Matches[2] ($form + '=')
        & $keepTyped
      }
    } else {
      & $addFlags $typed
    }
    return $results
  }

  if (-not $endOfOptions -and $position -eq 0) {
    foreach ($sub in $context.subcommands) {
      & $add $sub.name 'Command' $sub.description $typed ''
    }
  }

  $arguments = $context.arguments
  if ($position -lt $arguments.Count) {
    & $addValues $arguments[$position] $typed ''
  } elseif ($arguments.Count -gt 0 -and $arguments[-1].variadic) {
    & $addValues $arguments[-1] $typed ''
  }

  if (-not $endOfOptions) {
    & $addFlags $typed
  }
  return $results
}

###-end-t3-completions-###
