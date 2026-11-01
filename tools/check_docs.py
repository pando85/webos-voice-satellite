#!/usr/bin/env python3
"""Offline repository structure checks with no dependencies."""
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
REQUIRED = [
    "README.md", "AGENTS.md", "SECURITY.md", "CONTRIBUTING.md",
    "docs/project/vision.md", "docs/project/requirements.md",
    "docs/project/roadmap.md", "docs/research/README.md",
    "docs/research/sources.md", "docs/experiments/README.md",
    "docs/experiments/TEMPLATE.md", "docs/compatibility/README.md",
    ".agents/skills/webos-research/SKILL.md",
    "tools/probe/probe.sh",
]
RESEARCH = [
    "01-platform-lifecycle.md", "02-audio-capture.md", "03-playback-aec.md",
    "04-activation.md", "05-home-assistant.md", "06-media.md",
    "07-deployment.md", "08-security-quality.md",
]
errors = []
for name in REQUIRED + [f"docs/research/{n}" for n in RESEARCH]:
    if not (ROOT / name).is_file():
        errors.append(f"missing: {name}")

for path in ROOT.rglob("*.md"):
    rel_parts = path.relative_to(ROOT).parts
    if any(part.startswith(".git") or part in {".venv", "artifacts", "node_modules", "target"} for part in rel_parts):
        continue
    content = path.read_text(encoding="utf-8")
    if not content.startswith("# ") and not (path.name == "SKILL.md" and content.startswith("---")):
        errors.append(f"missing title: {path.relative_to(ROOT)}")
    for link in re.findall(r"\]\(([^)#]+)(?:#[^)]*)?\)", content):
        if link.startswith(("https://", "http://", "mailto:")) or "://" in link:
            continue
        target = (path.parent / link).resolve()
        if not target.exists():
            errors.append(f"broken relative link: {path.relative_to(ROOT)} -> {link}")
if errors:
    print("\n".join(errors), file=sys.stderr)
    sys.exit(1)
print(f"OK: {len(REQUIRED) + len(RESEARCH)} required paths and Markdown links")
