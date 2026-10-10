#!/usr/bin/env python3
"""Fixture tracker behind the command-template adapter.

Usage: command_tracker.py <state.json> <op> <id>
State: {"spec": {"id", "title", "body"}, "tickets": [{"id", "title", "body",
"finished", "ready", "blockers": [ids], "comments": [..]}], "mode": "ok" | "fail" | "garble"}.
"""
import json
import sys

state_path, op, arg = sys.argv[1], sys.argv[2], sys.argv[3]
with open(state_path) as f:
    state = json.load(f)
if state.get("mode") == "fail":
    sys.exit("tracker is down")
if state.get("mode") == "garble":
    print("not json")
    sys.exit(0)


def save():
    with open(state_path, "w") as f:
        json.dump(state, f)


def ticket(id):
    for t in state["tickets"]:
        if t["id"] == id:
            return t
    sys.exit(f"no ticket {id}")


def spec(id):
    if state["spec"]["id"] != id:
        sys.exit(f"no spec {id}")
    return state["spec"]


if op == "get-spec":
    s = spec(arg)
    print(json.dumps({"title": s["title"], "body": s["body"]}))
elif op == "list-tickets":
    spec(arg)
    keys = ("id", "title", "body", "finished", "ready")
    print(json.dumps([{k: t[k] for k in keys} for t in state["tickets"]]))
elif op == "list-blockers":
    known = {t["id"]: t["finished"] for t in state["tickets"]}
    print(json.dumps([{"id": b, "finished": known.get(b)} for b in ticket(arg)["blockers"]]))
elif op == "mark-finished":
    ticket(arg)["finished"] = True
    save()
    print("{}")
elif op == "comment":
    ticket(arg).setdefault("comments", []).append(sys.stdin.read())
    save()
    print("{}")
elif op == "tick":
    t, wanted, found = ticket(arg), json.load(sys.stdin), set()
    lines = t["body"].split("\n")
    for n, line in enumerate(lines):
        if line[:6] in ("- [ ] ", "- [x] ") and line[6:] in wanted:
            found.add(line[6:])
            lines[n] = "- [x] " + line[6:]
    t["body"] = "\n".join(lines)
    save()
    print(json.dumps({"missing": [w for w in wanted if w not in found]}))
else:
    sys.exit(f"unknown op {op}")
