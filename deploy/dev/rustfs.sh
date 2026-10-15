#!/bin/sh
# Runs RustFS (Apache-2.0), the S3 server for local development and tests. Production uses Cloudflare R2.
#
#   deploy/dev/rustfs.sh install   download, verify and unpack the server binary
#   deploy/dev/rustfs.sh start     run it in the background on :9000 (console on :9001)
#   deploy/dev/rustfs.sh stop      stop the process recorded in rustfs.pid
#   deploy/dev/rustfs.sh status    say whether that process is alive and the port answers
#
# Everything lives in ~/.cache/goodfolk-rustfs: the binary, the data directory, rustfs.pid and rustfs.log.
# Needs curl, sha256sum and unzip.
set -eu

VERSION=1.0.1
ARCHIVE="rustfs-linux-x86_64-musl-v$VERSION.zip"
URL="https://github.com/rustfs/rustfs/releases/download/$VERSION/$ARCHIVE"
SHA256=a834096dafa1f1a55825a2cdaf49d006a193978d344f2d508c2be475133738a3

ACCESS_KEY=goodfolk
SECRET_KEY=goodfolk_dev_secret
PORT=9000

root="${HOME}/.cache/goodfolk-rustfs"
bin="$root/rustfs"
data="$root/data"
pidfile="$root/rustfs.pid"
logfile="$root/rustfs.log"

# The pid must be alive and still be RustFS: after a reboot a stale pidfile can name an unrelated process.
alive() {
    [ -f "$pidfile" ] && [ "$(ps -p "$(cat "$pidfile")" -o comm= 2>/dev/null)" = rustfs ]
}

answers() {
    # Any HTTP response, even 403, means the server is up.
    curl -s -o /dev/null --max-time 2 "http://localhost:$PORT/"
}

install() {
    if [ -x "$bin" ]; then
        echo "RustFS $VERSION already installed in $root"
        return
    fi
    mkdir -p "$root"
    work=$(mktemp -d "$root/unpack.XXXXXX")
    trap 'rm -rf "$work"' EXIT
    curl -fsSL -o "$work/$ARCHIVE" "$URL"
    echo "$SHA256  $work/$ARCHIVE" | sha256sum -c --quiet
    unzip -q "$work/$ARCHIVE" -d "$work/x"
    rm "$work/$ARCHIVE"
    cp "$work/x/rustfs" "$bin"
    chmod 755 "$bin"
    echo "RustFS $VERSION installed in $root"
}

start() {
    if alive; then
        echo "RustFS already running (pid $(cat "$pidfile"))"
        return
    fi
    [ -x "$bin" ] || { echo "RustFS is not installed: run $0 install" >&2; exit 1; }
    mkdir -p "$data"
    RUSTFS_ACCESS_KEY=$ACCESS_KEY RUSTFS_SECRET_KEY=$SECRET_KEY \
        nohup "$bin" server "$data" --address ":$PORT" --console-enable --console-address ":9001" \
        >"$logfile" 2>&1 </dev/null &
    echo $! >"$pidfile"
    i=0
    while ! answers; do
        i=$((i + 1))
        if [ "$i" -gt 60 ] || ! alive; then
            echo "RustFS did not come up; see $logfile" >&2
            exit 1
        fi
        sleep 0.5
    done
    echo "RustFS running (pid $(cat "$pidfile")) on :$PORT, console on :9001"
}

stop() {
    if alive; then
        kill "$(cat "$pidfile")"
        echo "RustFS stopped"
    else
        echo "RustFS not running"
    fi
    rm -f "$pidfile"
}

status() {
    if alive && answers; then
        echo "RustFS running (pid $(cat "$pidfile")), answering on :$PORT"
    elif alive; then
        echo "RustFS process $(cat "$pidfile") is alive but :$PORT does not answer" >&2
        exit 1
    else
        echo "RustFS not running" >&2
        exit 1
    fi
}

case "${1:-}" in
    install) install ;;
    start) start ;;
    stop) stop ;;
    status) status ;;
    *) echo "usage: $0 install|start|stop|status" >&2; exit 2 ;;
esac
