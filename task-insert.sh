#!/usr/bin/env bash
set -euo pipefail

OWNER="dknedlik"
REPO="ai-proxy"
PROJECT_NUM="1"
CSV="tasks.csv"

echo "Importing tasks from $CSV into $OWNER/$REPO project #$PROJECT_NUM..."

# requires: gh (logged in), python3
python3 - "$OWNER" "$REPO" "$PROJECT_NUM" "$CSV" <<'PY'
import csv, os, sys, subprocess

owner, repo, proj, csv_path = sys.argv[1:5]
count = 0

with open(csv_path, newline='') as f:
    reader = csv.DictReader(f)
    for row_num, row in enumerate(reader, start=2):
        if not row or not row.get("Title") or not row["Title"].strip():
            print(f"Warning: Skipping empty or missing Title at row {row_num}", file=sys.stderr)
            continue

        title = row["Title"].strip()
        body = (row.get("Body") or "").replace("\\n","\n")
        labels = [l.strip() for l in (row.get("Labels") or "").split(",") if l.strip()]

        cmd = ["gh","issue","create","--repo",f"{owner}/{repo}","--title",title,"--body",body]
        for lab in labels:
            cmd += ["--label", lab]
        try:
            issue_url = subprocess.check_output(cmd, text=True).strip()
        except subprocess.CalledProcessError as e:
            print(f"Error creating issue for '{title}': {e}", file=sys.stderr)
            continue

        try:
            subprocess.run(["gh","project","item-add",proj,"--owner",owner,"--url",issue_url], check=True)
        except subprocess.CalledProcessError as e:
            print(f"Error adding issue to project for '{title}': {e}", file=sys.stderr)
            continue

        print(f"Created and added: {issue_url}")
        count += 1

print(f"Total issues created and added: {count}")
PY