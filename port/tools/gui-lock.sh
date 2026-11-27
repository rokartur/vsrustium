# Sourced by the Studio tools that put windows on screen, so parallel sessions do not take each other's desktop.
# Every launch takes focus (the port's cx.activate, Electron's window focus), so no tool that opens a window runs bare.
# Exclusive (the default; also takeGuiLock in the bun tools): the run sends input, needs focus or the pasteboard, moves
# the pointer or measures memory: ui-pair, a11y-check, shell-gui-check (cua-driver and System Events input), pixel-check
# (pointer warp, typed keys), bench (memory after a settle time), ref-commands (fixed CDP port), api-tests (window tests
# skip without focus), settings-check and dump-settings (results changed beside other runs); extension-tests,
# app-smoke and run-app-check, not proven beside others.
# Shared (port_gui_shared=1 before sourcing): runs whose windows only need to exist, same results in 3 rounds beside
# each other: webview-check, shell-check, extensions-check, privacy-check, network-capture. Never beside an exclusive run.
# The lock is the directory /tmp/port-gui.lock (PORT_GUI_LOCK moves it) holding the owner pid; a dead owner's lock is
# taken over. A shared group keeps the directory with one file per member in shared/ and pid naming a live member, so
# tools that only know the directory wait for it. No one joins a group while an exclusive run waits or after 10 min.
port_gui_lock=${PORT_GUI_LOCK:-/tmp/port-gui.lock}
port_gui_shared=${port_gui_shared:-}
port_gui_waiting=

# Runs "$@" holding $port_gui_lock.mutex, which orders the joins and leaves of a shared group.
port_gui_mutexed() {
  local owner code
  until mkdir "$port_gui_lock.mutex" 2>/dev/null; do
    owner=$(cat "$port_gui_lock.mutex/pid" 2>/dev/null || true)
    if [[ -n $owner ]] && ! kill -0 "$owner" 2>/dev/null; then
      rm -rf "$port_gui_lock.mutex"
    fi
    sleep 0.1
  done
  echo $$ >"$port_gui_lock.mutex/pid"
  "$@" && code=0 || code=$?
  rm -rf "$port_gui_lock.mutex"
  return "$code"
}

port_gui_try() {
  local member live= waiter
  if [[ -d $port_gui_lock/shared ]]; then
    # Members killed without their EXIT trap leave their file.
    for member in "$port_gui_lock"/shared/*; do
      if [[ -e $member ]] && kill -0 "${member##*/}" 2>/dev/null; then
        live=${member##*/}
      else
        rm -f "$member"
      fi
    done
    if [[ -z $live ]]; then
      rm -rf "$port_gui_lock"
    else
      echo "$live" >"$port_gui_lock/pid"
    fi
  fi
  if [[ -n $port_gui_shared ]]; then
    waiter=$(cat "$port_gui_lock.want" 2>/dev/null || true)
    if [[ -n $waiter ]] && kill -0 "$waiter" 2>/dev/null; then
      return 1
    fi
    if [[ -n $live ]] && (($(date +%s) - $(stat -f %B "$port_gui_lock/shared") < 600)); then
      touch "$port_gui_lock/shared/$$"
      return 0
    fi
  fi
  mkdir "$port_gui_lock" 2>/dev/null || return 1
  echo $$ >"$port_gui_lock/pid"
  if [[ -n $port_gui_shared ]]; then
    mkdir "$port_gui_lock/shared" && touch "$port_gui_lock/shared/$$"
  fi
  return 0
}

port_gui_leave() {
  rm -f "$port_gui_lock/shared/$$"
  local rest=("$port_gui_lock"/shared/*)
  if [[ -e ${rest[0]} ]]; then
    echo "${rest[0]##*/}" >"$port_gui_lock/pid"
  else
    rm -rf "$port_gui_lock"
  fi
}

until port_gui_mutexed port_gui_try; do
  owner=$(cat "$port_gui_lock/pid" 2>/dev/null || true)
  if [[ -n $owner && ! -d $port_gui_lock/shared ]] && ! kill -0 "$owner" 2>/dev/null; then
    rm -rf "$port_gui_lock"
    continue
  fi
  [[ -n $port_gui_shared ]] || echo $$ >"$port_gui_lock.want"
  # A run that waits here for another session looks hung to a caller's timeout: say so once.
  if [[ -z $port_gui_waiting ]]; then
    echo "gui-lock: waiting for pid $owner ($(ps -o command= -p "$owner" 2>/dev/null | cut -c1-80))" >&2
    port_gui_waiting=1
  fi
  sleep 5
done
if [[ -n $port_gui_shared ]]; then
  trap 'port_gui_mutexed port_gui_leave' EXIT
else
  rm -f "$port_gui_lock.want"
  trap 'rm -rf "$port_gui_lock"' EXIT
fi
trap 'exit 1' INT TERM
