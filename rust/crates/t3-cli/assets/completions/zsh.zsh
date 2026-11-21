#compdef t3
###-begin-t3-completions-###
#
# Static completion script for Zsh
#
# Installation:
#   t3 --completions zsh > ~/.zsh/completions/_t3
#   then add ~/.zsh/completions to your fpath
#

_t3() {
  local context state state_descr line
  typeset -A opt_args

  local -a commands
  commands=(
    'help:Show command help.'
    'serve:Run T3 Code in this terminal, with no browser.'
    'app:Open a project in the running T3 Code desktop app.'
    'pair:Mint a pairing token for a running T3 Code server and print it as a QR code.'
    'auth:Manage the local auth control plane for headless deployments.'
    'environment:Link other T3 Code environments, so agents here can work there.'
    'browser:Manage T3'\''s headless browser on this host.'
    'project:Manage projects.'
    'thread:Start, message, and read threads on the running server.'
    'up:Set up remote access and the T3 Code background service.'
    'status:Show service setup and remote access settings.'
    'service:Manage the T3 Code background service.'
    'update:Download a newer t3 and switch this machine to it, including the background service when one is installed.'
    'install:Install this version as the `t3` command.'
    'uninstall:Remove t3 from this machine\: the background service, the launcher, and every downloaded version. Your projects and threads are kept.'
    'theme:Inspect and set environment-wide theme defaults.'
    'trace:Inspect the local server trace file.'
    'doctor:Inspect this T3 Code data directory and repair it. Pauses and restarts the background service as needed.'
    'policy:Inspect the administrator'\''s managed policy.'
    'triage:Investigate a T3 Code problem on this machine with Claude Code, Codex, or Antigravity, and help file a good issue.'
  )

  local -a specs
  specs=(
    '(--mode)--mode[Runtime mode. `desktop` keeps loopback defaults unless overridden.]:value:(web desktop)'
    '(--port)--port[Port for the HTTP/WebSocket server.]:integer:'
    '(--host)--host[Host/interface to bind (for example 127.0.0.1, 0.0.0.0, or a Tailnet IP).]:value:'
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--dev-url)--dev-url[Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).]:value:'
    '(--no-browser --no-no-browser)--no-browser[Disable automatic browser opening.]'
    '(--no-browser --no-no-browser)--no-no-browser[Disable no-browser]'
    '(--bootstrap-fd)--bootstrap-fd[Read one-time bootstrap secrets from the given file descriptor.]:integer:'
    '(--auto-bootstrap-project-from-cwd --no-auto-bootstrap-project-from-cwd)--auto-bootstrap-project-from-cwd[Create a project for the current working directory on startup when missing.]'
    '(--auto-bootstrap-project-from-cwd --no-auto-bootstrap-project-from-cwd)--no-auto-bootstrap-project-from-cwd[Disable auto-bootstrap-project-from-cwd]'
    '(--log-websocket-events --log-ws-events --no-log-websocket-events)--log-websocket-events[Emit server-side logs for outbound WebSocket push traffic (equivalent to T3CODE_LOG_WS_EVENTS).]'
    '(--log-websocket-events --log-ws-events --no-log-websocket-events)--log-ws-events[Emit server-side logs for outbound WebSocket push traffic (equivalent to T3CODE_LOG_WS_EVENTS).]'
    '(--log-websocket-events --log-ws-events --no-log-websocket-events)--no-log-websocket-events[Disable log-websocket-events]'
    '(--tailscale-serve --no-tailscale-serve)--tailscale-serve[Configure Tailscale Serve to expose this backend over HTTPS on the Tailnet.]'
    '(--tailscale-serve --no-tailscale-serve)--no-tailscale-serve[Disable tailscale-serve]'
    '(--tailscale-serve-port)--tailscale-serve-port[HTTPS port for Tailscale Serve when --tailscale-serve is enabled.]:integer:'
    '(--lan --no-lan)--lan[Accept connections from your local network and tailnet, not only this machine (equivalent to T3CODE_LAN).]'
    '(--lan --no-lan)--no-lan[Disable lan]'
    -
    parent-arguments
    ':Working directory for provider sessions (defaults to the current directory).:'
    -
    subcommands
    '1:command:->command'
    '*::arg:->args'
  )

  _arguments -C "${specs[@]}"

  case "$state" in
    command)
      _describe -t commands 'commands' commands
      ;;
    args)
      case "$words[1]" in
        help)
          _t3_help
          ;;
        serve)
          _t3_serve
          ;;
        app)
          _t3_app
          ;;
        pair)
          _t3_pair
          ;;
        auth)
          _t3_auth
          ;;
        environment)
          _t3_environment
          ;;
        browser)
          _t3_browser
          ;;
        project)
          _t3_project
          ;;
        thread)
          _t3_thread
          ;;
        up)
          _t3_up
          ;;
        status)
          _t3_status
          ;;
        service)
          _t3_service
          ;;
        update)
          _t3_update
          ;;
        install)
          _t3_install
          ;;
        uninstall)
          _t3_uninstall
          ;;
        theme)
          _t3_theme
          ;;
        trace)
          _t3_trace
          ;;
        doctor)
          _t3_doctor
          ;;
        policy)
          _t3_policy
          ;;
        triage)
          _t3_triage
          ;;
      esac
      ;;
  esac
}

_t3_help() {
}

_t3_serve() {
  local -a specs
  specs=(
    '(--mode)--mode[Runtime mode. `desktop` keeps loopback defaults unless overridden.]:value:(web desktop)'
    '(--port)--port[Port for the HTTP/WebSocket server.]:integer:'
    '(--host)--host[Host/interface to bind (for example 127.0.0.1, 0.0.0.0, or a Tailnet IP).]:value:'
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--dev-url)--dev-url[Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).]:value:'
    '(--no-browser --no-no-browser)--no-browser[Disable automatic browser opening.]'
    '(--no-browser --no-no-browser)--no-no-browser[Disable no-browser]'
    '(--bootstrap-fd)--bootstrap-fd[Read one-time bootstrap secrets from the given file descriptor.]:integer:'
    '(--log-websocket-events --log-ws-events --no-log-websocket-events)--log-websocket-events[Emit server-side logs for outbound WebSocket push traffic (equivalent to T3CODE_LOG_WS_EVENTS).]'
    '(--log-websocket-events --log-ws-events --no-log-websocket-events)--log-ws-events[Emit server-side logs for outbound WebSocket push traffic (equivalent to T3CODE_LOG_WS_EVENTS).]'
    '(--log-websocket-events --log-ws-events --no-log-websocket-events)--no-log-websocket-events[Disable log-websocket-events]'
    '(--tailscale-serve --no-tailscale-serve)--tailscale-serve[Configure Tailscale Serve to expose this backend over HTTPS on the Tailnet.]'
    '(--tailscale-serve --no-tailscale-serve)--no-tailscale-serve[Disable tailscale-serve]'
    '(--tailscale-serve-port)--tailscale-serve-port[HTTPS port for Tailscale Serve when --tailscale-serve is enabled.]:integer:'
    '(--lan --no-lan)--lan[Accept connections from your local network and tailnet, not only this machine (equivalent to T3CODE_LAN).]'
    '(--lan --no-lan)--no-lan[Disable lan]'
    ':Working directory for provider sessions (defaults to the current directory).:'
  )
  _arguments "${specs[@]}"
}

_t3_app() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    ':Project directory. Default\: current directory.:'
  )
  _arguments "${specs[@]}"
}

_t3_pair() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '*--scope[Authorization scope to grant. Repeat for multiple scopes; replaces the default set\: orchestration\:read, orchestration\:operate, settings\:write, providers\:manage, environment\:maintain, preview\:operate, diagnostics\:read, terminal\:read, terminal\:operate, source-control\:write, filesystem\:read, filesystem\:write, relay\:read, access\:read, access\:write, relay\:write.]:value:(orchestration\:read orchestration\:operate settings\:write providers\:manage environment\:maintain preview\:operate diagnostics\:read terminal\:read terminal\:operate filesystem\:read filesystem\:write source-control\:write access\:read access\:write relay\:read relay\:write relay\:configure)'
    '(--ttl)--ttl[Token TTL, for example `5m`, `1h`, or `15 minutes`. Defaults to 5 minutes.]:value:'
    '(--label)--label[Optional label shown in the server'\''s connections list.]:value:'
    '(--tailscale --no-tailscale)--tailscale[Publish the server over Tailscale Serve HTTPS and pair through the tailnet URL.]'
    '(--tailscale --no-tailscale)--no-tailscale[Disable tailscale]'
    '(--tailscale-serve-port)--tailscale-serve-port[HTTPS port for Tailscale Serve when --tailscale is enabled.]:integer:'
  )
  _arguments "${specs[@]}"
}

_t3_auth() {
  local context state state_descr line
  typeset -A opt_args

  local -a commands
  commands=(
    'pairing:Manage one-time client pairing tokens.'
    'session:Manage bearer sessions.'
    'connect-device:Approve and revoke devices that reach this host through T3 Connect.'
  )

  local -a specs
  specs=(
    '1:command:->command'
    '*::arg:->args'
  )

  _arguments -C "${specs[@]}"

  case "$state" in
    command)
      _describe -t commands 'commands' commands
      ;;
    args)
      case "$words[1]" in
        pairing)
          _t3_auth_pairing
          ;;
        session)
          _t3_auth_session
          ;;
        connect-device)
          _t3_auth_connect_device
          ;;
      esac
      ;;
  esac
}

_t3_auth_pairing() {
  local context state state_descr line
  typeset -A opt_args

  local -a commands
  commands=(
    'create:Issue a new client pairing token.'
    'list:List active client pairing tokens without revealing their secrets.'
    'revoke:Revoke an active client pairing token.'
  )

  local -a specs
  specs=(
    '1:command:->command'
    '*::arg:->args'
  )

  _arguments -C "${specs[@]}"

  case "$state" in
    command)
      _describe -t commands 'commands' commands
      ;;
    args)
      case "$words[1]" in
        create)
          _t3_auth_pairing_create
          ;;
        list)
          _t3_auth_pairing_list
          ;;
        revoke)
          _t3_auth_pairing_revoke
          ;;
      esac
      ;;
  esac
}

_t3_auth_pairing_create() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--dev-url)--dev-url[Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).]:value:'
    '*--scope[Authorization scope to grant. Repeat for multiple scopes; replaces the default set\: orchestration\:read, orchestration\:operate, settings\:write, providers\:manage, environment\:maintain, preview\:operate, diagnostics\:read, terminal\:read, terminal\:operate, source-control\:write, filesystem\:read, filesystem\:write, relay\:read, access\:read, access\:write, relay\:write.]:value:(orchestration\:read orchestration\:operate settings\:write providers\:manage environment\:maintain preview\:operate diagnostics\:read terminal\:read terminal\:operate filesystem\:read filesystem\:write source-control\:write access\:read access\:write relay\:read relay\:write relay\:configure)'
    '(--ttl)--ttl[TTL, for example `5m`, `1h`, `30d`, or `15 minutes`.]:value:'
    '(--label)--label[Optional human-readable label.]:value:'
    '(--base-url)--base-url[Optional public base URL used to print a ready `/pair#token=...` link.]:value:'
    '(--json --no-json)--json[Emit JSON instead of human-readable output.]'
    '(--json --no-json)--no-json[Disable json]'
  )
  _arguments "${specs[@]}"
}

_t3_auth_pairing_list() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--dev-url)--dev-url[Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).]:value:'
    '(--json --no-json)--json[Emit JSON instead of human-readable output.]'
    '(--json --no-json)--no-json[Disable json]'
  )
  _arguments "${specs[@]}"
}

_t3_auth_pairing_revoke() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--dev-url)--dev-url[Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).]:value:'
    ':Pairing credential id to revoke.:'
  )
  _arguments "${specs[@]}"
}

_t3_auth_session() {
  local context state state_descr line
  typeset -A opt_args

  local -a commands
  commands=(
    'issue:Issue a scoped bearer access token for headless or remote clients.'
    'list:List active sessions without revealing bearer tokens.'
    'revoke:Revoke an active session. A T3 Connect device'\''s session takes the device'\''s trust and other sessions with it; if approval for new devices is off, the device can connect again on its own.'
  )

  local -a specs
  specs=(
    '1:command:->command'
    '*::arg:->args'
  )

  _arguments -C "${specs[@]}"

  case "$state" in
    command)
      _describe -t commands 'commands' commands
      ;;
    args)
      case "$words[1]" in
        issue)
          _t3_auth_session_issue
          ;;
        list)
          _t3_auth_session_list
          ;;
        revoke)
          _t3_auth_session_revoke
          ;;
      esac
      ;;
  esac
}

_t3_auth_session_issue() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--dev-url)--dev-url[Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).]:value:'
    '*--scope[Authorization scope to grant. Repeat for multiple scopes; replaces the default set\: orchestration\:read, orchestration\:operate, settings\:write, providers\:manage, environment\:maintain, preview\:operate, diagnostics\:read, terminal\:read, terminal\:operate, source-control\:write, filesystem\:read, filesystem\:write, relay\:read, access\:read, access\:write, relay\:write.]:value:(orchestration\:read orchestration\:operate settings\:write providers\:manage environment\:maintain preview\:operate diagnostics\:read terminal\:read terminal\:operate filesystem\:read filesystem\:write source-control\:write access\:read access\:write relay\:read relay\:write relay\:configure)'
    '(--ttl)--ttl[TTL, for example `5m`, `1h`, `30d`, or `15 minutes`.]:value:'
    '(--label)--label[Optional human-readable label.]:value:'
    '(--subject)--subject[Optional session subject.]:value:'
    '(--token-only --no-token-only)--token-only[Print only the issued bearer token.]'
    '(--token-only --no-token-only)--no-token-only[Disable token-only]'
    '(--json --no-json)--json[Emit JSON instead of human-readable output.]'
    '(--json --no-json)--no-json[Disable json]'
  )
  _arguments "${specs[@]}"
}

_t3_auth_session_list() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--dev-url)--dev-url[Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).]:value:'
    '(--json --no-json)--json[Emit JSON instead of human-readable output.]'
    '(--json --no-json)--no-json[Disable json]'
  )
  _arguments "${specs[@]}"
}

_t3_auth_session_revoke() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--dev-url)--dev-url[Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).]:value:'
    ':Session id to revoke.:'
  )
  _arguments "${specs[@]}"
}

_t3_auth_connect_device() {
  local context state state_descr line
  typeset -A opt_args

  local -a commands
  commands=(
    'list:List T3 Connect devices waiting for approval and those already trusted.'
    'approve:Approve the waiting T3 Connect device that shows this code.'
    'revoke:Stop trusting a T3 Connect device and end its sessions.'
  )

  local -a specs
  specs=(
    '1:command:->command'
    '*::arg:->args'
  )

  _arguments -C "${specs[@]}"

  case "$state" in
    command)
      _describe -t commands 'commands' commands
      ;;
    args)
      case "$words[1]" in
        list)
          _t3_auth_connect_device_list
          ;;
        approve)
          _t3_auth_connect_device_approve
          ;;
        revoke)
          _t3_auth_connect_device_revoke
          ;;
      esac
      ;;
  esac
}

_t3_auth_connect_device_list() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--dev-url)--dev-url[Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).]:value:'
    '(--json --no-json)--json[Emit JSON instead of human-readable output.]'
    '(--json --no-json)--no-json[Disable json]'
  )
  _arguments "${specs[@]}"
}

_t3_auth_connect_device_approve() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--dev-url)--dev-url[Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).]:value:'
    ':The code the device shows, such as `K7QM-2XFD`.:'
  )
  _arguments "${specs[@]}"
}

_t3_auth_connect_device_revoke() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--dev-url)--dev-url[Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).]:value:'
    ':The code the device shows, such as `K7QM-2XFD`.:'
  )
  _arguments "${specs[@]}"
}

_t3_environment() {
  local context state state_descr line
  typeset -A opt_args

  local -a commands
  commands=(
    'link:Link another T3 Code environment, so agents here can work there. It shows up there under Authorized clients in Settings → Sharing & access, where it can be revoked.'
    'list:List linked environments and whether each answers now.'
    'unlink:Forget a linked environment here. Revoke it in that environment'\''s Sharing & access to end its session there too.'
  )

  local -a specs
  specs=(
    '1:command:->command'
    '*::arg:->args'
  )

  _arguments -C "${specs[@]}"

  case "$state" in
    command)
      _describe -t commands 'commands' commands
      ;;
    args)
      case "$words[1]" in
        link)
          _t3_environment_link
          ;;
        list)
          _t3_environment_list
          ;;
        unlink)
          _t3_environment_unlink
          ;;
      esac
      ;;
  esac
}

_t3_environment_link() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--dev-url)--dev-url[Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).]:value:'
    '*--url[Another address for the same environment, tried after the first.]:value:'
    '(--pairing-code)--pairing-code[A pairing code from the other environment (`t3 pair` there).]:value:'
    '(--access)--access[What this environment'\''s agents may do there\: read-only, or the broadest runtime mode.]:value:(read-only approval-required auto-accept-edits auto full-access)'
    ':Where the other environment answers\: its T3 Connect, Tailscale, LAN or loopback address.:'
  )
  _arguments "${specs[@]}"
}

_t3_environment_list() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--dev-url)--dev-url[Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).]:value:'
    '(--json --no-json)--json[Emit JSON instead of human-readable output.]'
    '(--json --no-json)--no-json[Disable json]'
  )
  _arguments "${specs[@]}"
}

_t3_environment_unlink() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--dev-url)--dev-url[Dev web URL to proxy/redirect to (equivalent to VITE_DEV_SERVER_URL).]:value:'
    ':The linked environment'\''s id, from `t3 environment list`.:'
  )
  _arguments "${specs[@]}"
}

_t3_browser() {
  local context state state_descr line
  typeset -A opt_args

  local -a commands
  commands=(
    'setup:Set up this Linux host for T3'\''s browser\: allow Chrome'\''s sandbox and install its libraries and fonts.'
  )

  local -a specs
  specs=(
    '1:command:->command'
    '*::arg:->args'
  )

  _arguments -C "${specs[@]}"

  case "$state" in
    command)
      _describe -t commands 'commands' commands
      ;;
    args)
      case "$words[1]" in
        setup)
          _t3_browser_setup
          ;;
      esac
      ;;
  esac
}

_t3_browser_setup() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
  )
  _arguments "${specs[@]}"
}

_t3_project() {
  local context state state_descr line
  typeset -A opt_args

  local -a commands
  commands=(
    'add:Add a project.'
    'remove:Remove a project.'
    'rename:Rename a project.'
  )

  local -a specs
  specs=(
    '1:command:->command'
    '*::arg:->args'
  )

  _arguments -C "${specs[@]}"

  case "$state" in
    command)
      _describe -t commands 'commands' commands
      ;;
    args)
      case "$words[1]" in
        add)
          _t3_project_add
          ;;
        remove)
          _t3_project_remove
          ;;
        rename)
          _t3_project_rename
          ;;
      esac
      ;;
  esac
}

_t3_project_add() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--title)--title[Optional project title.]:value:'
    ':Workspace root to add as a project.:'
  )
  _arguments "${specs[@]}"
}

_t3_project_remove() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--force --no-force)--force[Delete the project and all of its threads.]'
    '(--force --no-force)--no-force[Disable force]'
    ':Project id or workspace root to remove.:'
  )
  _arguments "${specs[@]}"
}

_t3_project_rename() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    ':Project id or workspace root to rename.:'
    ':New project title.:'
  )
  _arguments "${specs[@]}"
}

_t3_thread() {
  local context state state_descr line
  typeset -A opt_args

  local -a commands
  commands=(
    'list:List a project'\''s threads, newest first.'
    'start:Start a thread in a project and print its id.'
    'send:Send a message to a thread. A busy thread gets it as its next turn; nothing is interrupted.'
    'show:Show a thread'\''s state and its latest messages.'
  )

  local -a specs
  specs=(
    '1:command:->command'
    '*::arg:->args'
  )

  _arguments -C "${specs[@]}"

  case "$state" in
    command)
      _describe -t commands 'commands' commands
      ;;
    args)
      case "$words[1]" in
        list)
          _t3_thread_list
          ;;
        start)
          _t3_thread_start
          ;;
        send)
          _t3_thread_send
          ;;
        show)
          _t3_thread_show
          ;;
      esac
      ;;
  esac
}

_t3_thread_list() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--project)--project[Project id or a path inside the project. Defaults to this directory.]:value:'
    '(--json --no-json)--json[Print JSON instead of text.]'
    '(--json --no-json)--no-json[Disable json]'
  )
  _arguments "${specs[@]}"
}

_t3_thread_start() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--title)--title[Thread title. Defaults to the message'\''s first line.]:value:'
    '(--provider)--provider[Provider instance id. Pass with --model.]:value:'
    '(--model)--model[Model slug. Defaults to the project'\''s default model.]:value:'
    '*--option[Model option as id=value, such as effort=high. Repeat for more. Needs --provider and --model.]:value:'
    '(--worktree)--worktree[Run in a new git worktree based on this branch, tag, or commit, instead of the project folder.]:value:'
    '(--branch)--branch[Branch name for the new worktree.]:value:'
    '(--wait --no-wait)--wait[Wait for the agent to finish and print its reply. Exits 1 if the run fails and 2 if it waits for an answer.]'
    '(--wait --no-wait)--no-wait[Disable wait]'
    '(--json --no-json)--json[Print JSON instead of text.]'
    '(--json --no-json)--no-json[Disable json]'
    ':Project id or a path inside the project, such as .:'
    ':Message for the agent, or - to read it from stdin.:'
  )
  _arguments "${specs[@]}"
}

_t3_thread_send() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--wait --no-wait)--wait[Wait for the agent to finish and print its reply. Exits 1 if the run fails and 2 if it waits for an answer.]'
    '(--wait --no-wait)--no-wait[Disable wait]'
    '(--json --no-json)--json[Print JSON instead of text.]'
    '(--json --no-json)--no-json[Disable json]'
    ':Thread id.:'
    ':Message for the agent, or - to read it from stdin.:'
  )
  _arguments "${specs[@]}"
}

_t3_thread_show() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--messages)--messages[How many of the latest messages to print.]:integer:'
    '(--json --no-json)--json[Print JSON instead of text.]'
    '(--json --no-json)--no-json[Disable json]'
    ':Thread id.:'
  )
  _arguments "${specs[@]}"
}

_t3_up() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--connect --no-connect)--connect[Allow access through T3 Connect with your T3 account. Turn off with --no-connect.]'
    '(--connect --no-connect)--no-connect[Disable connect]'
    '(--lan --no-lan)--lan[Allow LAN and tailnet connections. Anyone there can reach the port; a pairing code is the only lock. Turn off with --no-lan.]'
    '(--lan --no-lan)--no-lan[Disable lan]'
    '(--yes --no-yes)--yes[Ask nothing. Keep the access settings; a new machine gets T3 Connect. A restart stops running agent turns.]'
    '(--yes --no-yes)--no-yes[Disable yes]'
    '(--headless --no-headless)--headless[Sign in with a URL and code on any device. Automatic without a terminal or over SSH.]'
    '(--headless --no-headless)--no-headless[Disable headless]'
  )
  _arguments "${specs[@]}"
}

_t3_status() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--json --no-json)--json[Print the settings as JSON. Does not test connections.]'
    '(--json --no-json)--no-json[Disable json]'
  )
  _arguments "${specs[@]}"
}

_t3_service() {
  local context state state_descr line
  typeset -A opt_args

  local -a commands
  commands=(
    'install:Install T3 Code as a background service for this user. `t3 up` also chooses how your other devices reach it.'
    'restart:Restart the background service. Picks up a version installed by `t3 update` that was not restarted at the time.'
    'uninstall:Stop and remove the T3 Code background service.'
    'status:Show whether the T3 Code background service is installed.'
    'protect:Protect the T3 Code server from agents under memory pressure (Linux with systemd). Asks for sudo.'
  )

  local -a specs
  specs=(
    '1:command:->command'
    '*::arg:->args'
  )

  _arguments -C "${specs[@]}"

  case "$state" in
    command)
      _describe -t commands 'commands' commands
      ;;
    args)
      case "$words[1]" in
        install)
          _t3_service_install
          ;;
        restart)
          _t3_service_restart
          ;;
        uninstall)
          _t3_service_uninstall
          ;;
        status)
          _t3_service_status
          ;;
        protect)
          _t3_service_protect
          ;;
      esac
      ;;
  esac
}

_t3_service_install() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--allow-downgrade --no-allow-downgrade)--allow-downgrade[Allow replacing a newer installed service with this older CLI version.]'
    '(--allow-downgrade --no-allow-downgrade)--no-allow-downgrade[Disable allow-downgrade]'
  )
  _arguments "${specs[@]}"
}

_t3_service_restart() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
  )
  _arguments "${specs[@]}"
}

_t3_service_uninstall() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
  )
  _arguments "${specs[@]}"
}

_t3_service_status() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
  )
  _arguments "${specs[@]}"
}

_t3_service_protect() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--undo --no-undo)--undo[Remove the protection this command set up.]'
    '(--undo --no-undo)--no-undo[Disable undo]'
    '(--yes --no-yes)--yes[Answer yes to every question, including SSH and Tailscale.]'
    '(--yes --no-yes)--no-yes[Disable yes]'
  )
  _arguments "${specs[@]}"
}

_t3_update() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--channel)--channel[Release channel to follow. Defaults to the channel this t3 was published on.]:value:(stable nightly preview)'
    '(--allow-downgrade --no-allow-downgrade)--allow-downgrade[Allow moving to an older version than the one running.]'
    '(--allow-downgrade --no-allow-downgrade)--no-allow-downgrade[Disable allow-downgrade]'
    '(--yes -y --no-yes)--yes[Restart the background service without asking. Required to restart it from a script, where there is no prompt.]'
    '(--yes -y --no-yes)-y[Restart the background service without asking. Required to restart it from a script, where there is no prompt.]'
    '(--yes -y --no-yes)--no-yes[Disable yes]'
    ':Exact version to install. Defaults to the newest release on the channel.:'
  )
  _arguments "${specs[@]}"
}

_t3_install() {
}

_t3_uninstall() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--yes -y --no-yes)--yes[Remove everything without asking. Required from a script, where there is no prompt.]'
    '(--yes -y --no-yes)-y[Remove everything without asking. Required from a script, where there is no prompt.]'
    '(--yes -y --no-yes)--no-yes[Disable yes]'
  )
  _arguments "${specs[@]}"
}

_t3_theme() {
  local context state state_descr line
  typeset -A opt_args

  local -a commands
  commands=(
    'set:Set the environment'\''s theme. The web app this server serves and the desktop app on this machine switch to it; a desktop app connected from another machine does not.'
    'clear:Remove the environment'\''s theme; clients keep what they have.'
    'show:Show the environment'\''s theme and its published themes.'
  )

  local -a specs
  specs=(
    '1:command:->command'
    '*::arg:->args'
  )

  _arguments -C "${specs[@]}"

  case "$state" in
    command)
      _describe -t commands 'commands' commands
      ;;
    args)
      case "$words[1]" in
        set)
          _t3_theme_set
          ;;
        clear)
          _t3_theme_clear
          ;;
        show)
          _t3_theme_show
          ;;
      esac
      ;;
  esac
}

_t3_theme_set() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--id)--id[Theme id to publish a file under, instead of its filename.]:value:'
    ':A theme id (a built-in, or one this machine publishes, where themes/nightfall.json is "nightfall"), or a path to a theme JSON file to publish and set in one step.:'
  )
  _arguments "${specs[@]}"
}

_t3_theme_clear() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
  )
  _arguments "${specs[@]}"
}

_t3_theme_show() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
  )
  _arguments "${specs[@]}"
}

_t3_trace() {
  local context state state_descr line
  typeset -A opt_args

  local -a commands
  commands=(
    'summary:Summarize the local server trace file\: counts, rates, and latency.'
  )

  local -a specs
  specs=(
    '1:command:->command'
    '*::arg:->args'
  )

  _arguments -C "${specs[@]}"

  case "$state" in
    command)
      _describe -t commands 'commands' commands
      ;;
    args)
      case "$words[1]" in
        summary)
          _t3_trace_summary
          ;;
      esac
      ;;
  esac
}

_t3_trace_summary() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--since)--since[Only count spans that ended in this window, for example 30m or 2h.]:value:'
    '(--limit)--limit[Number of span names to show, busiest first.]:integer:'
  )
  _arguments "${specs[@]}"
}

_t3_doctor() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--fix --no-fix)--fix[Apply the recommended repairs without prompting.]'
    '(--fix --no-fix)--no-fix[Disable fix]'
    '(--dry-run --no-dry-run)--dry-run[Show the repairs without changing anything, even with --fix.]'
    '(--dry-run --no-dry-run)--no-dry-run[Disable dry-run]'
  )
  _arguments "${specs[@]}"
}

_t3_policy() {
  local context state state_descr line
  typeset -A opt_args

  local -a commands
  commands=(
    'show:Show the policy installed on this machine, and whether the running server loaded it.'
    'check:Validate a policy file before deploying it.'
    'templates:Write the Windows Group Policy template and a macOS configuration profile.'
  )

  local -a specs
  specs=(
    '1:command:->command'
    '*::arg:->args'
  )

  _arguments -C "${specs[@]}"

  case "$state" in
    command)
      _describe -t commands 'commands' commands
      ;;
    args)
      case "$words[1]" in
        show)
          _t3_policy_show
          ;;
        check)
          _t3_policy_check
          ;;
        templates)
          _t3_policy_templates
          ;;
      esac
      ;;
  esac
}

_t3_policy_show() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
  )
  _arguments "${specs[@]}"
}

_t3_policy_check() {
  local -a specs
  specs=(
    ':Policy file to validate.:'
  )
  _arguments "${specs[@]}"
}

_t3_policy_templates() {
  local -a specs
  specs=(
    '(--policy)--policy[Policy file to put in the macOS profile. Default\: a small sample.]:value:'
    ':Directory to write the templates to.:'
  )
  _arguments "${specs[@]}"
}

_t3_triage() {
  local -a specs
  specs=(
    '(--base-dir)--base-dir[Explicit T3 Code data directory; runtime state is stored under userdata (equivalent to T3CODE_HOME).]:value:'
    '(--agent)--agent[Agent CLI to use. Default\: ask when multiple are installed.]:value:(claude codex antigravity)'
    '(--model)--model[Model passed through to the agent CLI. Default\: the agent'\''s default.]:value:'
  )
  _arguments "${specs[@]}"
}

# Handle both direct invocation and autoload
if [[ "${zsh_eval_context[-1]}" == "loadautofunc" ]]; then
  _t3 "$@"
else
  compdef _t3 t3
fi
###-end-t3-completions-###
