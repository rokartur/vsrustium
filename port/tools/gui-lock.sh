# Sourced by the Studio tools that put windows on screen or measure memory, so parallel sessions take turns.
# Takes /tmp/port-gui.lock (a directory holding the owner pid); a lock whose owner died is taken over.
# Every launch takes focus (port main.rs cx.activate, Electron's window focus), so tools that open a window cannot run
# beside each other: api-tests skips a window test without focus, settings-check and dump-settings changed results.
port_gui_lock=/tmp/port-gui.lock
port_gui_waiting=
until mkdir "$port_gui_lock" 2>/dev/null; do
  owner=$(cat "$port_gui_lock/pid" 2>/dev/null || true)
  if [[ -n $owner ]] && ! kill -0 "$owner" 2>/dev/null; then
    rm -rf "$port_gui_lock"
    continue
  fi
  # A run that waits here for another session looks hung to a caller's timeout: say so once.
  if [[ -z $port_gui_waiting ]]; then
    echo "gui-lock: waiting for pid $owner ($(ps -o command= -p "$owner" 2>/dev/null | cut -c1-80))" >&2
    port_gui_waiting=1
  fi
  sleep 5
done
echo $$ > "$port_gui_lock/pid"
trap 'rm -rf "$port_gui_lock"' EXIT
trap 'exit 1' INT TERM
