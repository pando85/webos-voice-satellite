#!/usr/bin/env python3
"""C7 of the setup server review: no command the phone setup server may run
(`set ...`, `mic ...`, `pair`, `status`, `tv-settings`) reaches the code that
ends a setup session or the daemon (stop_daemon, stop_setup_server, the stop,
restart, install, uninstall and purge commands), directly or through any
function of ctl.sh and lib.sh that they call.

usage: no_self_stop.py CTL_SH LIB_SH
Prints one line per root with the size of its call closure; exit 1 on a hit,
and also when a root is missing or the checker finds nothing in a positive
control (cmd_stop must be seen reaching stop_daemon)."""
import re
import sys


def functions(text):
    funcs = {}
    for m in re.finditer(r"^([A-Za-z_][A-Za-z_0-9]*)\(\) *\{.*?^\}", text, re.S | re.M):
        funcs[m.group(1)] = m.group(0)
    return funcs


def strip_comments(body):
    out = []
    for line in body.splitlines():
        if re.match(r"^\s*#", line):
            continue
        out.append(line)
    return "\n".join(out)


def closure(funcs, root):
    seen, todo = set(), [root]
    while todo:
        f = todo.pop()
        if f in seen or f not in funcs:
            continue
        seen.add(f)
        body = strip_comments(funcs[f])
        for name in funcs:
            if name != f and re.search(r"(?<![A-Za-z_0-9-])" + re.escape(name) + r"(?![A-Za-z_0-9-])", body):
                todo.append(name)
    return seen


# Words that end a session or the daemon. start.sh/stop.sh are the scripts
# behind them.
FORBIDDEN = [
    r"stop_daemon", r"stop_setup_server", r"\bstop\.sh\b", r"\bstart\.sh\b",
    r"\buninstall\.sh\b", r"\binstall\.sh\b", r"\bcmd_stop\b", r"\bcmd_start\b",
    r"\bcmd_uninstall\b", r"\bcmd_purge\b", r"\bcmd_install\b", r"\bcmd_setup_stop\b",
    r"\$SETUP_DIR/stop", r"\bSETUP_DIR\b.*\brm\b", r"\brm\b.*\bSETUP_DIR\b",
]
ROOTS = ["cmd_set", "cmd_mic", "cmd_trigger", "cmd_status", "cmd_tv_settings"]


def hits(funcs, root):
    found = []
    for f in sorted(closure(funcs, root)):
        body = strip_comments(funcs[f])
        for pat in FORBIDDEN:
            if re.search(pat, body):
                found.append((f, pat))
    return found


def main():
    funcs = {}
    for path in sys.argv[1:3]:
        funcs.update(functions(open(path).read()))
    bad = 0
    for root in ROOTS:
        if root not in funcs:
            print(f"FAIL: {root} not found")
            bad += 1
            continue
        h = hits(funcs, root)
        print(f"{root}: {len(closure(funcs, root))} functions, {len(h)} hits")
        for f, pat in h:
            print(f"  {f} matches {pat}")
            bad += 1
    # Positive control: the checker must see what it is looking for.
    if not hits(funcs, "cmd_stop"):
        print("FAIL: positive control: cmd_stop was not seen reaching stop_daemon")
        bad += 1
    if not hits(funcs, "cmd_setup_stop"):
        print("FAIL: positive control: cmd_setup_stop was not seen reaching stop_setup_server")
        bad += 1
    sys.exit(1 if bad else 0)


main()
