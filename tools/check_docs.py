#!/usr/bin/env python3
"""Offline repository structure checks with no dependencies."""
from pathlib import Path
import re
import sys
import tomllib

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


def verified_table_rows(text):
    """(series, firmware) of the rows of the "## Verified" table."""
    section = re.search(r"^## Verified\s*$(.*?)(?=^## |\Z)", text, re.M | re.S)
    rows = []
    if not section:
        return rows
    for line in section.group(1).splitlines():
        if not line.startswith("|"):
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        # Header, separator and empty rows carry no firmware version.
        if len(cells) < 4 or not re.fullmatch(r"`[^`]+`", cells[3]):
            continue
        rows.append((cells[0], cells[3].strip("`")))
    return rows


def verified_data_rows(text):
    """(series name, firmware) of data/verified-models.toml."""
    data = tomllib.loads(text)
    return [(s["name"], f["version"]) for s in data.get("series", []) for f in s.get("firmware", [])]


def check_verified_list(root):
    """The Verified table of the docs and the data file must list the same rows."""
    md = root / "docs/compatibility/verified-models.md"
    toml_path = root / "data/verified-models.toml"
    if not md.is_file() or not toml_path.is_file():
        return [f"missing: {p.relative_to(root)}" for p in (md, toml_path) if not p.is_file()]
    try:
        table = verified_table_rows(md.read_text(encoding="utf-8"))
        data = verified_data_rows(toml_path.read_text(encoding="utf-8"))
    except (tomllib.TOMLDecodeError, KeyError) as e:
        return [f"data/verified-models.toml: {e}"]
    if sorted(table) != sorted(data):
        return [
            "verified list differs: docs/compatibility/verified-models.md has "
            f"{sorted(table)} but data/verified-models.toml has {sorted(data)}"
        ]
    return []


FORM_IDS = {"report", "details", "comments", "confirm"}


def check_issue_form(root):
    """The compatibility report form parses and has the ids the link prefills."""
    path = root / ".github/ISSUE_TEMPLATE/compatibility-report.yml"
    if not path.is_file():
        return [f"missing: {path.relative_to(root)}"]
    text = path.read_text(encoding="utf-8")
    errs = []
    try:
        import yaml  # optional: used when installed, a regex check otherwise
    except ImportError:
        yaml = None
    if yaml is not None:
        try:
            form = yaml.safe_load(text)
        except yaml.YAMLError as e:
            return [f"compatibility-report.yml does not parse: {e}"]
        body = form.get("body", []) if isinstance(form, dict) else []
        ids = {b.get("id") for b in body if isinstance(b, dict) and b.get("id")}
        # Fields are required through `validations`; checkboxes per option.
        required = {
            b["id"] for b in body
            if isinstance(b, dict) and b.get("id") and (
                (b.get("validations") or {}).get("required")
                or any(o.get("required") for o in (b.get("attributes") or {}).get("options", []))
            )
        }
        labels = form.get("labels") if isinstance(form, dict) else None
        if labels != ["compat-report"]:
            errs.append("compatibility-report.yml: labels must be [compat-report]")
        for key in ("name", "description", "body"):
            if not isinstance(form, dict) or key not in form:
                errs.append(f"compatibility-report.yml: missing {key}")
    else:
        ids = set(re.findall(r"^\s+id:\s*(\S+)\s*$", text, re.M))
        required = set()
        if not re.search(r'^labels:\s*\["compat-report"\]\s*$', text, re.M):
            errs.append("compatibility-report.yml: labels must be [compat-report]")
        if "required: true" not in text:
            errs.append("compatibility-report.yml: no required field")
    if ids != FORM_IDS:
        errs.append(f"compatibility-report.yml: ids are {sorted(ids)}, expected {sorted(FORM_IDS)}")
    if yaml is not None and not {"report", "confirm"} <= required:
        errs.append("compatibility-report.yml: report and confirm must be required")
    return errs


if __name__ == "__main__":
    errors += check_verified_list(ROOT)
    errors += check_issue_form(ROOT)
    if errors:
        print("\n".join(errors), file=sys.stderr)
        sys.exit(1)
    print(f"OK: {len(REQUIRED) + len(RESEARCH)} required paths, Markdown links, verified list and issue form")
