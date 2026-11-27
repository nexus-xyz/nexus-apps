#!/usr/bin/env python3
"""Check Chromium's sandbox on Linux: start the app's web host (`<binary> --nexus-web-host`), open one page, run
JavaScript in it over CDP, then report every Chromium child process's sandbox state from /proc.

Usage: web-tests/sandbox/probe.py BINARY OUT.json
Run it in a throwaway HOME (`env -i HOME=$(mktemp -d) PATH=/usr/bin:/bin python3 -I ...`). To see a system without
user namespaces, run that under `bwrap --dev-bind / / --unshare-user --disable-userns --`.

What it proves, per child process (`--type=zygote`, `renderer`, `gpu-process`, `utility`):
  - `no_sandbox_flag`: Chromium passed `--no-sandbox` on its command line (the sandbox is off).
  - `seccomp`: /proc/<pid>/status `Seccomp` (2 = a seccomp-bpf filter is on, Chromium's renderer sandbox).
  - `own_user_ns` / `own_pid_ns` / `own_net_ns`: the process sits in a namespace the host does not (the namespace
    sandbox).
And for the page: the host's `ready`, `title`, `frame` events, and the CDP `Runtime.evaluate` answer.
Run it in a throwaway HOME; it writes nothing but OUT.json.
"""
import json
import os
import subprocess
import sys
import threading
import time

PAGE = ("data:text/html,<title>sandbox-probe</title><h1 style='font:40px sans-serif'>sandbox probe</h1>"
        "<script>document.title='sandbox-probe-js'</script>")


def children(root):
    """Every descendant pid of `root`, from /proc's parent links."""
    parents = {}
    for d in os.listdir("/proc"):
        if d.isdigit():
            try:
                with open(f"/proc/{d}/stat") as f:
                    parents[int(d)] = int(f.read().rsplit(")", 1)[1].split()[1])
            except OSError:
                pass
    out, todo = [], [root]
    while todo:
        p = todo.pop()
        for c, pp in parents.items():
            if pp == p:
                out.append(c)
                todo.append(c)
    return out


def proc_info(pid, host_ns):
    def read(path):
        try:
            with open(path, "rb") as f:
                return f.read()
        except OSError:
            return b""
    argv = [a.decode(errors="replace") for a in read(f"/proc/{pid}/cmdline").split(b"\0") if a]
    status = dict(l.split(":\t", 1) for l in read(f"/proc/{pid}/status").decode().splitlines() if ":\t" in l)
    ns = {k: os.readlink(f"/proc/{pid}/ns/{k}") if os.path.exists(f"/proc/{pid}/ns/{k}") else None
          for k in ("user", "pid", "net")}
    kind = next((a.split("=", 1)[1] for a in argv if a.startswith("--type=")), "?")
    return {
        "pid": pid,
        "type": kind,
        "no_sandbox_flag": "--no-sandbox" in argv,
        "seccomp": status.get("Seccomp", "?").strip(),
        "no_new_privs": status.get("NoNewPrivs", "?").strip(),
        "own_user_ns": ns["user"] is not None and ns["user"] != host_ns["user"],
        "own_pid_ns": ns["pid"] is not None and ns["pid"] != host_ns["pid"],
        "own_net_ns": ns["net"] is not None and ns["net"] != host_ns["net"],
    }


def main():
    binary, out_path = sys.argv[1], sys.argv[2]
    err = open(out_path + ".stderr.log", "wb")
    host = subprocess.Popen([binary, "--nexus-web-host"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=err)
    events, lock = [], threading.Lock()

    def reader():
        for line in host.stdout:
            try:
                e = json.loads(line)
            except ValueError:
                continue
            with lock:
                events.append(e)
            if e.get("event") == "frame":
                send({"cmd": "ack", "id": e["id"]})

    def send(cmd):
        try:
            host.stdin.write((json.dumps(cmd) + "\n").encode())
            host.stdin.flush()
        except (BrokenPipeError, ValueError):
            pass

    threading.Thread(target=reader, daemon=True).start()
    send({"cmd": "open", "id": 1, "url": PAGE, "width": 640, "height": 360, "scale": 1.0, "frame_rate": 30})

    def seen(kind):
        with lock:
            return [e for e in events if e.get("event") == kind]

    deadline = time.time() + 30
    while time.time() < deadline and not (seen("frame") and seen("title")):
        if host.poll() is not None:
            break
        time.sleep(0.2)
    send({"cmd": "cdp", "id": 1, "message": json.dumps(
        {"id": 7, "method": "Runtime.evaluate",
         "params": {"expression": "document.title + ' / ' + (6*7)", "returnByValue": True}})})
    time.sleep(2)

    host_ns = {k: os.readlink(f"/proc/{host.pid}/ns/{k}") for k in ("user", "pid", "net")} \
        if host.poll() is None else {"user": None, "pid": None, "net": None}
    procs = [proc_info(p, host_ns) for p in children(host.pid)] if host.poll() is None else []
    with lock:
        cdp = [e for e in events if e.get("event") == "cdp" and '"id":7' in e.get("message", "")]
        result = {
            "binary": binary,
            "host_alive": host.poll() is None,
            "host_exit": host.returncode,
            "ready": [e.get("ok") for e in events if e.get("event") == "ready"],
            "titles": [e["title"] for e in events if e.get("event") == "title"],
            "frames": len([e for e in events if e.get("event") == "frame"]),
            "crashed": len([e for e in events if e.get("event") == "crashed"]),
            "cdp_answer": json.loads(cdp[0]["message"]).get("result") if cdp else None,
            "children": procs,
        }
    with open(out_path, "w") as f:
        json.dump(result, f, indent=2)
    print(json.dumps(result, indent=2))
    send({"cmd": "close", "id": 1})
    try:
        host.stdin.close()
    except BrokenPipeError:
        pass
    try:
        host.wait(timeout=10)
    except subprocess.TimeoutExpired:
        host.kill()


if __name__ == "__main__":
    main()
