#!/usr/bin/env python3
"""PreToolUse guard: block AI attribution in commit / PR-authoring commands.

DWD policy (CLAUDE.md): never add AI attribution anywhere — no
"Co-Authored-By: Claude", no "Generated with Claude Code", no Anthropic footer.
This hook is the project-level override of the harness default that otherwise
appends such a footer to commit messages.

It reads the PreToolUse event JSON on stdin. If the Bash command is a commit /
PR / release authoring command whose text carries an attribution marker, it
denies the tool call; otherwise it stays silent and exits 0 (normal flow). Any
parse problem is treated as "allow" so the hook never breaks unrelated shell use.

Limitation: only the command string is visible. A message supplied via a file
(`git commit -F file`) cannot be inspected here — the skill instructions and code
review are the backstop for that path.
"""

import json
import re
import sys

# Only scrutinise commands that actually author commit / PR / release text.
# `git ... commit` tolerates global options between `git` and `commit`
# (e.g. `git -C path commit`, `git -c user.name=x commit`); [^&|;] keeps the
# match inside a single command segment so piped/chained reads don't false-match.
AUTHORING = re.compile(
    r"\bgit\b[^&|;]*\bcommit\b"
    r"|\bgh\s+pr\s+(create|edit|comment|review|merge)\b"
    r"|\bgh\s+release\s+create\b",
    re.IGNORECASE,
)

# Attribution markers — specific enough to avoid false positives on legitimate
# messages that merely mention Claude/Anthropic.
ATTRIBUTION = re.compile(
    r"co-authored-by:\s*claude"
    r"|co-developed with claude"
    r"|generated with \[?claude"
    r"|🤖 generated"
    r"|claude\.com/claude-code"
    r"|noreply@anthropic\.com",
    re.IGNORECASE,
)


def main() -> int:
    try:
        event = json.load(sys.stdin)
    except (json.JSONDecodeError, ValueError):
        return 0  # malformed input -> don't interfere

    command = (event.get("tool_input") or {}).get("command", "")
    if not isinstance(command, str) or not command:
        return 0

    if AUTHORING.search(command) and ATTRIBUTION.search(command):
        decision = {
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": (
                    "AI attribution is prohibited by project policy (CLAUDE.md). "
                    "Remove any 'Co-Authored-By: Claude', 'Generated with Claude Code', "
                    "or Anthropic attribution from the commit/PR text and retry."
                ),
            }
        }
        print(json.dumps(decision))
    return 0


if __name__ == "__main__":
    sys.exit(main())
