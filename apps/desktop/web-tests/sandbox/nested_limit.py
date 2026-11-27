#!/usr/bin/env python3
"""Run a command where a process can make ONE more user namespace but not a nested one.

It enters a new user namespace (same uid and gid inside), sets `user.max_user_namespaces` to 1 there, and execs the
command. The command's first `unshare(CLONE_NEWUSER)` then works and a nested one fails: the case where a bare
`unshare` check passes but Chromium's `CanCreateProcessInNewUserNS` fails (as with Ubuntu's AppArmor userns limit).
Usage: web-tests/sandbox/nested_limit.py CMD [ARGS...], for example `nested_limit.py python3 -I web-tests/sandbox/probe.py BINARY OUT.json`
"""
import os
import sys

uid, gid = os.getuid(), os.getgid()
os.unshare(os.CLONE_NEWUSER)
with open("/proc/self/setgroups", "w") as f:
    f.write("deny")
with open("/proc/self/uid_map", "w") as f:
    f.write(f"{uid} {uid} 1")
with open("/proc/self/gid_map", "w") as f:
    f.write(f"{gid} {gid} 1")
with open("/proc/sys/user/max_user_namespaces", "w") as f:
    f.write("1")
os.execvp(sys.argv[1], sys.argv[1:])
