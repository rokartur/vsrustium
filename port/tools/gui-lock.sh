# Sourced by the Studio tools that put windows on screen or measure memory, so parallel sessions take turns.
# Takes /tmp/port-gui.lock (a directory holding the owner pid); a lock whose owner died is taken over.
port_gui_lock=/tmp/port-gui.lock
until mkdir "$port_gui_lock" 2>/dev/null; do
  owner=$(cat "$port_gui_lock/pid" 2>/dev/null || true)
  if [[ -n $owner ]] && ! kill -0 "$owner" 2>/dev/null; then
    rm -rf "$port_gui_lock"
    continue
  fi
  sleep 5
done
echo $$ > "$port_gui_lock/pid"
trap 'rm -rf "$port_gui_lock"' EXIT
trap 'exit 1' INT TERM
