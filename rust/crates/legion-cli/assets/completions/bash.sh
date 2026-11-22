###-begin-legioncode-completions-###
#
# Static completion script for Bash
#
# Installation:
#   legioncode --completions bash >> ~/.bashrc
#

if ! type _init_completion &>/dev/null; then
  _init_completion()
  {
    COMPREPLY=()
    local _i _j=0 _piece _line="$COMP_LINE"
    words=("${COMP_WORDS[0]}")
    cword=0
    _line="${_line#*"${COMP_WORDS[0]}"}"
    for ((_i = 1; _i < ${#COMP_WORDS[@]}; _i++)); do
      _piece="${COMP_WORDS[_i]}"
      if [[ "$_line" == [[:blank:]]* ]]; then
        ((_j++))
        words[_j]="$_piece"
      else
        words[_j]="${words[_j]}$_piece"
      fi
      ((_i == COMP_CWORD)) && cword=$_j
      _line="${_line#*"$_piece"}"
    done
    cur="${words[cword]}"
    prev=""
    ((cword > 0)) && prev="${words[cword-1]}"
    return 0
  }
fi

_legioncode--choices()
{
  local _cur="$1" _word="$2"; shift 2

  local _head="${_cur%"$_word"}"
  local _open=""
  case "$_head" in
    *\') _open="'" ;;
    *\") _open='"' ;;
  esac

  local _prefix="$_cur" _committed="$_head"
  _prefix=${_prefix//\\/}; _prefix=${_prefix//\"/}; _prefix=${_prefix//\'/}
  _committed=${_committed//\\/}; _committed=${_committed//\"/}; _committed=${_committed//\'/}

  COMPREPLY=()
  local _choice _rest _match
  for _choice in "$@"; do
    [[ "$_choice" == "$_prefix"* ]] || continue
    _rest="${_choice#"$_committed"}"
    case "$_open" in
      "'")
        if [[ "$_head" == "'" ]]; then
          _match=${_rest//\'/\'\\\'\'}
        else
          [[ "$_rest" == *\'* ]] && continue
          _match="$_rest"
        fi
        ;;
      '"')
        _match="${_rest//\\/\\\\}"
        _match="${_match//\$/\\$}"
        _match="${_match//\`/\\\`}"
        _match="${_match//\"/\\\"}"
        ;;
      *)
        printf -v _match '%q' "$_rest"
        [[ -z "$_head" && "$_match" == '~'* ]] && _match="\\$_match"
        ;;
    esac
    [[ -n "$_open" && "$_match" == *"$_open" ]] && _match+="$_open"
    COMPREPLY+=("$_match")
  done
}

_legioncode--paths()
{
  local _kind="$1" _cur="$2" _word="$3"

  local _prefix="$_cur" _committed="${_cur%"$_word"}"
  _prefix=${_prefix//\\/}; _prefix=${_prefix//\"/}; _prefix=${_prefix//\'/}
  _committed=${_committed//\\/}; _committed=${_committed//\"/}; _committed=${_committed//\'/}

  local _typed="" _dir="$_committed" _open="" _i
  if [[ "$_word" == */* ]]; then
    _typed="${_word%/*}/"
    _dir="${_prefix%/*}/"
    if [[ "$_cur" == "$_word" ]]; then
      for ((_i = 0; _i < ${#_typed}; _i++)); do
        case "$_open${_typed:_i:1}" in
          \\|\"\\) ((_i += 1)) ;;
          \"|\') _open="${_typed:_i:1}" ;;
          \"\"|\'\') _open="" ;;
        esac
      done
    fi
  fi

  local _path _names=() _rest="${_word#"$_typed"}"
  while IFS= read -r _path; do
    if [[ "$_path" == "$_dir"* ]]; then
      _names+=("$_committed${_path#"$_dir"}")
    elif [[ -z "$_committed" ]]; then
      _names+=("$_path")
      _typed="" _rest="" _open=""
    fi
  done < <(compgen "$_kind" -- "$_prefix")
  if [[ -n "$_open" ]]; then
    _typed+="$_open"
    _rest="${_prefix##*/}"
  fi

  _legioncode--choices "${_cur%"$_word"}$_rest" "$_rest" "${_names[@]}"
  for ((_i = 0; _i < ${#COMPREPLY[@]}; _i++)); do
    COMPREPLY[_i]="$_typed${COMPREPLY[_i]}"
  done
}

_legioncode()
{
  local cur prev words cword i
  local _command_index=0
  _init_completion -n "$COMP_WORDBREAKS" || return
  local _comp_word="$2"

  # Flag value completions
  case "$prev" in
    --mode)
      _legioncode--choices "$cur" "$_comp_word" 'web' 'desktop'
      return
      ;;
  esac

  # Subcommand dispatch
  local cmd _skip_next=0
  for ((i = _command_index + 1; i < cword; i++)); do
    if (( _skip_next )); then
      _skip_next=0
      continue
    fi
    case "${words[i]}" in
      --mode) _skip_next=1 ;;
      --mode=*) ;;
      --port) _skip_next=1 ;;
      --port=*) ;;
      --host) _skip_next=1 ;;
      --host=*) ;;
      --base-dir) _skip_next=1 ;;
      --base-dir=*) ;;
      --dev-url) _skip_next=1 ;;
      --dev-url=*) ;;
      --bootstrap-fd) _skip_next=1 ;;
      --bootstrap-fd=*) ;;
      --tailscale-serve-port) _skip_next=1 ;;
      --tailscale-serve-port=*) ;;
      help)
        _legioncode_help "$i"
        return
        ;;
      serve)
        _legioncode_serve "$i"
        return
        ;;
      app)
        _legioncode_app "$i"
        return
        ;;
      pair)
        _legioncode_pair "$i"
        return
        ;;
      auth)
        _legioncode_auth "$i"
        return
        ;;
      environment)
        _legioncode_environment "$i"
        return
        ;;
      browser)
        _legioncode_browser "$i"
        return
        ;;
      project)
        _legioncode_project "$i"
        return
        ;;
      thread)
        _legioncode_thread "$i"
        return
        ;;
      up)
        _legioncode_up "$i"
        return
        ;;
      status)
        _legioncode_status "$i"
        return
        ;;
      service)
        _legioncode_service "$i"
        return
        ;;
      update)
        _legioncode_update "$i"
        return
        ;;
      install)
        _legioncode_install "$i"
        return
        ;;
      uninstall)
        _legioncode_uninstall "$i"
        return
        ;;
      theme)
        _legioncode_theme "$i"
        return
        ;;
      trace)
        _legioncode_trace "$i"
        return
        ;;
      doctor)
        _legioncode_doctor "$i"
        return
        ;;
      policy)
        _legioncode_policy "$i"
        return
        ;;
      triage)
        _legioncode_triage "$i"
        return
        ;;
    esac
  done

  local _used_0="" _used_1="" _used_2="" _used_3="" _used_4="" _used_5="" _used_6="" _used_7="" _used_8="" _used_9="" _used_10="" _used_11=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --mode) _used_0=1 ;;
      --port) _used_1=1 ;;
      --host) _used_2=1 ;;
      --base-dir) _used_3=1 ;;
      --dev-url) _used_4=1 ;;
      --no-browser|--no-no-browser) _used_5=1 ;;
      --bootstrap-fd) _used_6=1 ;;
      --auto-bootstrap-project-from-cwd|--no-auto-bootstrap-project-from-cwd) _used_7=1 ;;
      --log-websocket-events|--log-ws-events|--no-log-websocket-events) _used_8=1 ;;
      --tailscale-serve|--no-tailscale-serve) _used_9=1 ;;
      --tailscale-serve-port) _used_10=1 ;;
      --lan|--no-lan) _used_11=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --mode"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --port"
  [[ -n "$_used_2" ]] || _filtered_flags+=" --host"
  [[ -n "$_used_3" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_4" ]] || _filtered_flags+=" --dev-url"
  [[ -n "$_used_5" ]] || _filtered_flags+=" --no-browser --no-no-browser"
  [[ -n "$_used_6" ]] || _filtered_flags+=" --bootstrap-fd"
  [[ -n "$_used_7" ]] || _filtered_flags+=" --auto-bootstrap-project-from-cwd --no-auto-bootstrap-project-from-cwd"
  [[ -n "$_used_8" ]] || _filtered_flags+=" --log-websocket-events --log-ws-events --no-log-websocket-events"
  [[ -n "$_used_9" ]] || _filtered_flags+=" --tailscale-serve --no-tailscale-serve"
  [[ -n "$_used_10" ]] || _filtered_flags+=" --tailscale-serve-port"
  [[ -n "$_used_11" ]] || _filtered_flags+=" --lan --no-lan"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

  _legioncode--choices "$cur" "$_comp_word" 'help' 'serve' 'app' 'pair' 'auth' 'environment' 'browser' 'project' 'thread' 'up' 'status' 'service' 'update' 'install' 'uninstall' 'theme' 'trace' 'doctor' 'policy' 'triage'
}

_legioncode_help()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

}

_legioncode_serve()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
    --mode)
      _legioncode--choices "$cur" "$_comp_word" 'web' 'desktop'
      return
      ;;
  esac

  local _used_0="" _used_1="" _used_2="" _used_3="" _used_4="" _used_5="" _used_6="" _used_7="" _used_8="" _used_9="" _used_10=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --mode) _used_0=1 ;;
      --port) _used_1=1 ;;
      --host) _used_2=1 ;;
      --base-dir) _used_3=1 ;;
      --dev-url) _used_4=1 ;;
      --no-browser|--no-no-browser) _used_5=1 ;;
      --bootstrap-fd) _used_6=1 ;;
      --log-websocket-events|--log-ws-events|--no-log-websocket-events) _used_7=1 ;;
      --tailscale-serve|--no-tailscale-serve) _used_8=1 ;;
      --tailscale-serve-port) _used_9=1 ;;
      --lan|--no-lan) _used_10=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --mode"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --port"
  [[ -n "$_used_2" ]] || _filtered_flags+=" --host"
  [[ -n "$_used_3" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_4" ]] || _filtered_flags+=" --dev-url"
  [[ -n "$_used_5" ]] || _filtered_flags+=" --no-browser --no-no-browser"
  [[ -n "$_used_6" ]] || _filtered_flags+=" --bootstrap-fd"
  [[ -n "$_used_7" ]] || _filtered_flags+=" --log-websocket-events --log-ws-events --no-log-websocket-events"
  [[ -n "$_used_8" ]] || _filtered_flags+=" --tailscale-serve --no-tailscale-serve"
  [[ -n "$_used_9" ]] || _filtered_flags+=" --tailscale-serve-port"
  [[ -n "$_used_10" ]] || _filtered_flags+=" --lan --no-lan"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_app()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_pair()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
    --scope)
      _legioncode--choices "$cur" "$_comp_word" 'orchestration:read' 'orchestration:operate' 'settings:write' 'providers:manage' 'environment:maintain' 'preview:operate' 'diagnostics:read' 'terminal:read' 'terminal:operate' 'filesystem:read' 'filesystem:write' 'source-control:write' 'access:read' 'access:write' 'relay:read' 'relay:write' 'relay:configure'
      return
      ;;
  esac

  local _used_0="" _used_1="" _used_2="" _used_3="" _used_4="" _used_5=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --scope) _used_1=1 ;;
      --ttl) _used_2=1 ;;
      --label) _used_3=1 ;;
      --tailscale|--no-tailscale) _used_4=1 ;;
      --tailscale-serve-port) _used_5=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  _filtered_flags+=" --scope"
  [[ -n "$_used_2" ]] || _filtered_flags+=" --ttl"
  [[ -n "$_used_3" ]] || _filtered_flags+=" --label"
  [[ -n "$_used_4" ]] || _filtered_flags+=" --tailscale --no-tailscale"
  [[ -n "$_used_5" ]] || _filtered_flags+=" --tailscale-serve-port"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_auth()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Subcommand dispatch
  local cmd _skip_next=0
  for ((i = _command_index + 1; i < cword; i++)); do
    if (( _skip_next )); then
      _skip_next=0
      continue
    fi
    case "${words[i]}" in
      pairing)
        _legioncode_auth_pairing "$i"
        return
        ;;
      session)
        _legioncode_auth_session "$i"
        return
        ;;
      connect-device)
        _legioncode_auth_connect_device "$i"
        return
        ;;
    esac
  done

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    return
  fi

  _legioncode--choices "$cur" "$_comp_word" 'pairing' 'session' 'connect-device'
}

_legioncode_auth_pairing()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Subcommand dispatch
  local cmd _skip_next=0
  for ((i = _command_index + 1; i < cword; i++)); do
    if (( _skip_next )); then
      _skip_next=0
      continue
    fi
    case "${words[i]}" in
      create)
        _legioncode_auth_pairing_create "$i"
        return
        ;;
      list)
        _legioncode_auth_pairing_list "$i"
        return
        ;;
      revoke)
        _legioncode_auth_pairing_revoke "$i"
        return
        ;;
    esac
  done

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    return
  fi

  _legioncode--choices "$cur" "$_comp_word" 'create' 'list' 'revoke'
}

_legioncode_auth_pairing_create()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
    --scope)
      _legioncode--choices "$cur" "$_comp_word" 'orchestration:read' 'orchestration:operate' 'settings:write' 'providers:manage' 'environment:maintain' 'preview:operate' 'diagnostics:read' 'terminal:read' 'terminal:operate' 'filesystem:read' 'filesystem:write' 'source-control:write' 'access:read' 'access:write' 'relay:read' 'relay:write' 'relay:configure'
      return
      ;;
  esac

  local _used_0="" _used_1="" _used_2="" _used_3="" _used_4="" _used_5="" _used_6=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --dev-url) _used_1=1 ;;
      --scope) _used_2=1 ;;
      --ttl) _used_3=1 ;;
      --label) _used_4=1 ;;
      --base-url) _used_5=1 ;;
      --json|--no-json) _used_6=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --dev-url"
  _filtered_flags+=" --scope"
  [[ -n "$_used_3" ]] || _filtered_flags+=" --ttl"
  [[ -n "$_used_4" ]] || _filtered_flags+=" --label"
  [[ -n "$_used_5" ]] || _filtered_flags+=" --base-url"
  [[ -n "$_used_6" ]] || _filtered_flags+=" --json --no-json"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_auth_pairing_list()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0="" _used_1="" _used_2=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --dev-url) _used_1=1 ;;
      --json|--no-json) _used_2=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --dev-url"
  [[ -n "$_used_2" ]] || _filtered_flags+=" --json --no-json"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_auth_pairing_revoke()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0="" _used_1=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --dev-url) _used_1=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --dev-url"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_auth_session()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Subcommand dispatch
  local cmd _skip_next=0
  for ((i = _command_index + 1; i < cword; i++)); do
    if (( _skip_next )); then
      _skip_next=0
      continue
    fi
    case "${words[i]}" in
      issue)
        _legioncode_auth_session_issue "$i"
        return
        ;;
      list)
        _legioncode_auth_session_list "$i"
        return
        ;;
      revoke)
        _legioncode_auth_session_revoke "$i"
        return
        ;;
    esac
  done

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    return
  fi

  _legioncode--choices "$cur" "$_comp_word" 'issue' 'list' 'revoke'
}

_legioncode_auth_session_issue()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
    --scope)
      _legioncode--choices "$cur" "$_comp_word" 'orchestration:read' 'orchestration:operate' 'settings:write' 'providers:manage' 'environment:maintain' 'preview:operate' 'diagnostics:read' 'terminal:read' 'terminal:operate' 'filesystem:read' 'filesystem:write' 'source-control:write' 'access:read' 'access:write' 'relay:read' 'relay:write' 'relay:configure'
      return
      ;;
  esac

  local _used_0="" _used_1="" _used_2="" _used_3="" _used_4="" _used_5="" _used_6="" _used_7=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --dev-url) _used_1=1 ;;
      --scope) _used_2=1 ;;
      --ttl) _used_3=1 ;;
      --label) _used_4=1 ;;
      --subject) _used_5=1 ;;
      --token-only|--no-token-only) _used_6=1 ;;
      --json|--no-json) _used_7=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --dev-url"
  _filtered_flags+=" --scope"
  [[ -n "$_used_3" ]] || _filtered_flags+=" --ttl"
  [[ -n "$_used_4" ]] || _filtered_flags+=" --label"
  [[ -n "$_used_5" ]] || _filtered_flags+=" --subject"
  [[ -n "$_used_6" ]] || _filtered_flags+=" --token-only --no-token-only"
  [[ -n "$_used_7" ]] || _filtered_flags+=" --json --no-json"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_auth_session_list()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0="" _used_1="" _used_2=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --dev-url) _used_1=1 ;;
      --json|--no-json) _used_2=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --dev-url"
  [[ -n "$_used_2" ]] || _filtered_flags+=" --json --no-json"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_auth_session_revoke()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0="" _used_1=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --dev-url) _used_1=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --dev-url"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_auth_connect_device()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Subcommand dispatch
  local cmd _skip_next=0
  for ((i = _command_index + 1; i < cword; i++)); do
    if (( _skip_next )); then
      _skip_next=0
      continue
    fi
    case "${words[i]}" in
      list)
        _legioncode_auth_connect_device_list "$i"
        return
        ;;
      approve)
        _legioncode_auth_connect_device_approve "$i"
        return
        ;;
      revoke)
        _legioncode_auth_connect_device_revoke "$i"
        return
        ;;
    esac
  done

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    return
  fi

  _legioncode--choices "$cur" "$_comp_word" 'list' 'approve' 'revoke'
}

_legioncode_auth_connect_device_list()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0="" _used_1="" _used_2=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --dev-url) _used_1=1 ;;
      --json|--no-json) _used_2=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --dev-url"
  [[ -n "$_used_2" ]] || _filtered_flags+=" --json --no-json"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_auth_connect_device_approve()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0="" _used_1=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --dev-url) _used_1=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --dev-url"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_auth_connect_device_revoke()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0="" _used_1=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --dev-url) _used_1=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --dev-url"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_environment()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Subcommand dispatch
  local cmd _skip_next=0
  for ((i = _command_index + 1; i < cword; i++)); do
    if (( _skip_next )); then
      _skip_next=0
      continue
    fi
    case "${words[i]}" in
      link)
        _legioncode_environment_link "$i"
        return
        ;;
      list)
        _legioncode_environment_list "$i"
        return
        ;;
      unlink)
        _legioncode_environment_unlink "$i"
        return
        ;;
    esac
  done

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    return
  fi

  _legioncode--choices "$cur" "$_comp_word" 'link' 'list' 'unlink'
}

_legioncode_environment_link()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
    --access)
      _legioncode--choices "$cur" "$_comp_word" 'read-only' 'approval-required' 'auto-accept-edits' 'auto' 'full-access'
      return
      ;;
  esac

  local _used_0="" _used_1="" _used_2="" _used_3="" _used_4=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --dev-url) _used_1=1 ;;
      --url) _used_2=1 ;;
      --pairing-code) _used_3=1 ;;
      --access) _used_4=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --dev-url"
  _filtered_flags+=" --url"
  [[ -n "$_used_3" ]] || _filtered_flags+=" --pairing-code"
  [[ -n "$_used_4" ]] || _filtered_flags+=" --access"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_environment_list()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0="" _used_1="" _used_2=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --dev-url) _used_1=1 ;;
      --json|--no-json) _used_2=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --dev-url"
  [[ -n "$_used_2" ]] || _filtered_flags+=" --json --no-json"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_environment_unlink()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0="" _used_1=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --dev-url) _used_1=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --dev-url"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_browser()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Subcommand dispatch
  local cmd _skip_next=0
  for ((i = _command_index + 1; i < cword; i++)); do
    if (( _skip_next )); then
      _skip_next=0
      continue
    fi
    case "${words[i]}" in
      setup)
        _legioncode_browser_setup "$i"
        return
        ;;
    esac
  done

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    return
  fi

  _legioncode--choices "$cur" "$_comp_word" 'setup'
}

_legioncode_browser_setup()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_project()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Subcommand dispatch
  local cmd _skip_next=0
  for ((i = _command_index + 1; i < cword; i++)); do
    if (( _skip_next )); then
      _skip_next=0
      continue
    fi
    case "${words[i]}" in
      add)
        _legioncode_project_add "$i"
        return
        ;;
      remove)
        _legioncode_project_remove "$i"
        return
        ;;
      rename)
        _legioncode_project_rename "$i"
        return
        ;;
    esac
  done

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    return
  fi

  _legioncode--choices "$cur" "$_comp_word" 'add' 'remove' 'rename'
}

_legioncode_project_add()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0="" _used_1=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --title) _used_1=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --title"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_project_remove()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0="" _used_1=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --force|--no-force) _used_1=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --force --no-force"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_project_rename()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_thread()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Subcommand dispatch
  local cmd _skip_next=0
  for ((i = _command_index + 1; i < cword; i++)); do
    if (( _skip_next )); then
      _skip_next=0
      continue
    fi
    case "${words[i]}" in
      list)
        _legioncode_thread_list "$i"
        return
        ;;
      start)
        _legioncode_thread_start "$i"
        return
        ;;
      send)
        _legioncode_thread_send "$i"
        return
        ;;
      show)
        _legioncode_thread_show "$i"
        return
        ;;
    esac
  done

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    return
  fi

  _legioncode--choices "$cur" "$_comp_word" 'list' 'start' 'send' 'show'
}

_legioncode_thread_list()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0="" _used_1="" _used_2=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --project) _used_1=1 ;;
      --json|--no-json) _used_2=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --project"
  [[ -n "$_used_2" ]] || _filtered_flags+=" --json --no-json"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_thread_start()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0="" _used_1="" _used_2="" _used_3="" _used_4="" _used_5="" _used_6="" _used_7="" _used_8="" _used_9="" _used_10=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --title) _used_1=1 ;;
      --generate-title|--no-generate-title) _used_9=1 ;;
      --provider) _used_2=1 ;;
      --model) _used_3=1 ;;
      --option) _used_4=1 ;;
      --worktree) _used_5=1 ;;
      --branch) _used_6=1 ;;
      --output-schema) _used_10=1 ;;
      --wait|--no-wait) _used_7=1 ;;
      --json|--no-json) _used_8=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --title"
  [[ -n "$_used_9" ]] || _filtered_flags+=" --generate-title --no-generate-title"
  [[ -n "$_used_2" ]] || _filtered_flags+=" --provider"
  [[ -n "$_used_3" ]] || _filtered_flags+=" --model"
  _filtered_flags+=" --option"
  [[ -n "$_used_5" ]] || _filtered_flags+=" --worktree"
  [[ -n "$_used_6" ]] || _filtered_flags+=" --branch"
  [[ -n "$_used_10" ]] || _filtered_flags+=" --output-schema"
  [[ -n "$_used_7" ]] || _filtered_flags+=" --wait --no-wait"
  [[ -n "$_used_8" ]] || _filtered_flags+=" --json --no-json"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_thread_send()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0="" _used_1="" _used_2="" _used_3=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --output-schema) _used_3=1 ;;
      --wait|--no-wait) _used_1=1 ;;
      --json|--no-json) _used_2=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_3" ]] || _filtered_flags+=" --output-schema"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --wait --no-wait"
  [[ -n "$_used_2" ]] || _filtered_flags+=" --json --no-json"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_thread_show()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0="" _used_1="" _used_2=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --messages) _used_1=1 ;;
      --json|--no-json) _used_2=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --messages"
  [[ -n "$_used_2" ]] || _filtered_flags+=" --json --no-json"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_up()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0="" _used_1="" _used_2="" _used_3="" _used_4=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --connect|--no-connect) _used_1=1 ;;
      --lan|--no-lan) _used_2=1 ;;
      --yes|--no-yes) _used_3=1 ;;
      --headless|--no-headless) _used_4=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --connect --no-connect"
  [[ -n "$_used_2" ]] || _filtered_flags+=" --lan --no-lan"
  [[ -n "$_used_3" ]] || _filtered_flags+=" --yes --no-yes"
  [[ -n "$_used_4" ]] || _filtered_flags+=" --headless --no-headless"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_status()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0="" _used_1=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --json|--no-json) _used_1=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --json --no-json"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_service()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Subcommand dispatch
  local cmd _skip_next=0
  for ((i = _command_index + 1; i < cword; i++)); do
    if (( _skip_next )); then
      _skip_next=0
      continue
    fi
    case "${words[i]}" in
      install)
        _legioncode_service_install "$i"
        return
        ;;
      restart)
        _legioncode_service_restart "$i"
        return
        ;;
      uninstall)
        _legioncode_service_uninstall "$i"
        return
        ;;
      status)
        _legioncode_service_status "$i"
        return
        ;;
      protect)
        _legioncode_service_protect "$i"
        return
        ;;
    esac
  done

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    return
  fi

  _legioncode--choices "$cur" "$_comp_word" 'install' 'restart' 'uninstall' 'status' 'protect'
}

_legioncode_service_install()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0="" _used_1=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --allow-downgrade|--no-allow-downgrade) _used_1=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --allow-downgrade --no-allow-downgrade"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_service_restart()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_service_uninstall()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_service_status()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_service_protect()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0="" _used_1="" _used_2=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --undo|--no-undo) _used_1=1 ;;
      --yes|--no-yes) _used_2=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --undo --no-undo"
  [[ -n "$_used_2" ]] || _filtered_flags+=" --yes --no-yes"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_update()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
    --channel)
      _legioncode--choices "$cur" "$_comp_word" 'stable' 'nightly' 'preview'
      return
      ;;
  esac

  local _used_0="" _used_1="" _used_2="" _used_3=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --channel) _used_1=1 ;;
      --allow-downgrade|--no-allow-downgrade) _used_2=1 ;;
      --yes|-y|--no-yes) _used_3=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --channel"
  [[ -n "$_used_2" ]] || _filtered_flags+=" --allow-downgrade --no-allow-downgrade"
  [[ -n "$_used_3" ]] || _filtered_flags+=" --yes -y --no-yes"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_install()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

}

_legioncode_uninstall()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0="" _used_1=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --yes|-y|--no-yes) _used_1=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --yes -y --no-yes"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_theme()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Subcommand dispatch
  local cmd _skip_next=0
  for ((i = _command_index + 1; i < cword; i++)); do
    if (( _skip_next )); then
      _skip_next=0
      continue
    fi
    case "${words[i]}" in
      set)
        _legioncode_theme_set "$i"
        return
        ;;
      clear)
        _legioncode_theme_clear "$i"
        return
        ;;
      show)
        _legioncode_theme_show "$i"
        return
        ;;
    esac
  done

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    return
  fi

  _legioncode--choices "$cur" "$_comp_word" 'set' 'clear' 'show'
}

_legioncode_theme_set()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0="" _used_1=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --id) _used_1=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --id"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_theme_clear()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_theme_show()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_trace()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Subcommand dispatch
  local cmd _skip_next=0
  for ((i = _command_index + 1; i < cword; i++)); do
    if (( _skip_next )); then
      _skip_next=0
      continue
    fi
    case "${words[i]}" in
      summary)
        _legioncode_trace_summary "$i"
        return
        ;;
    esac
  done

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    return
  fi

  _legioncode--choices "$cur" "$_comp_word" 'summary'
}

_legioncode_trace_summary()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0="" _used_1="" _used_2=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --since) _used_1=1 ;;
      --limit) _used_2=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --since"
  [[ -n "$_used_2" ]] || _filtered_flags+=" --limit"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_doctor()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0="" _used_1="" _used_2=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --fix|--no-fix) _used_1=1 ;;
      --dry-run|--no-dry-run) _used_2=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --fix --no-fix"
  [[ -n "$_used_2" ]] || _filtered_flags+=" --dry-run --no-dry-run"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_policy()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Subcommand dispatch
  local cmd _skip_next=0
  for ((i = _command_index + 1; i < cword; i++)); do
    if (( _skip_next )); then
      _skip_next=0
      continue
    fi
    case "${words[i]}" in
      show)
        _legioncode_policy_show "$i"
        return
        ;;
      check)
        _legioncode_policy_check "$i"
        return
        ;;
      settings)
        _legioncode_policy_settings "$i"
        return
        ;;
      templates)
        _legioncode_policy_templates "$i"
        return
        ;;
    esac
  done

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    return
  fi

  _legioncode--choices "$cur" "$_comp_word" 'show' 'check' 'settings' 'templates'
}

_legioncode_policy_show()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_policy_check()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

}

_legioncode_policy_settings()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

}

_legioncode_policy_templates()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
  esac

  local _used_0=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --policy) _used_0=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --policy"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

_legioncode_triage()
{
  local cur prev words cword i
  local _command_index="$1"
  _init_completion -n "$COMP_WORDBREAKS" || return

  # Flag value completions
  case "$prev" in
    --agent)
      _legioncode--choices "$cur" "$_comp_word" 'claude' 'codex' 'antigravity'
      return
      ;;
  esac

  local _used_0="" _used_1="" _used_2=""
  for ((i = 1; i < cword; i++)); do
    case "${words[i]%%=*}" in
      --base-dir) _used_0=1 ;;
      --agent) _used_1=1 ;;
      --model) _used_2=1 ;;
    esac
  done
  local _filtered_flags=""
  [[ -n "$_used_0" ]] || _filtered_flags+=" --base-dir"
  [[ -n "$_used_1" ]] || _filtered_flags+=" --agent"
  [[ -n "$_used_2" ]] || _filtered_flags+=" --model"

  # Complete flags (filtered) and subcommands
  if [[ "$cur" == -* ]]; then
    COMPREPLY=( $(compgen -W "$_filtered_flags" -- "$cur") )
    return
  fi

}

complete -F _legioncode legioncode
###-end-legioncode-completions-###
