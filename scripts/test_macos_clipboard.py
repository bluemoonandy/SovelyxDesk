"""Verify the shipped clipboard commands against real AppKit on an isolated CI Mac."""

import json
import os
import shlex
from pathlib import Path
import re
import subprocess
import sys


def command(name):
    source = (Path(__file__).resolve().parent.parent / "src" / "files.rs").read_text(
        encoding="utf-8"
    )
    match = re.search(
        rf"const\s+{name}:\s*&str\s*=\s*r#\"(.*?)\"#;", source, re.DOTALL
    )
    if match is None:
        raise RuntimeError(f"Cannot locate the shipped {name} command")
    return match.group(1)


def invoke(script, data=b""):
    result = subprocess.run(
        ["/bin/sh", "-c", script],
        input=data,
        capture_output=True,
        timeout=20,
        check=False,
    )
    if result.returncode:
        raise RuntimeError(
            f"Native Mac clipboard helper exited {result.returncode}: "
            f"{result.stderr.decode('utf-8', errors='replace')}"
        )
    return result.stdout


def main():
    if sys.platform != "darwin" or os.environ.get("GITHUB_ACTIONS") != "true":
        raise RuntimeError(
            "Run this test only on an isolated GitHub Actions Mac. "
            "It deliberately replaces that test machine's clipboard."
        )
    read = command("READ_CLIPBOARD")
    write = command("WRITE_CLIPBOARD")
    samples = [
        "",
        "MacCanvas Unicode: caf\u00e9 \u03bb \U0001f34e\nsecond line",
        "{\\rtf1 This must remain literal plain text.}",
        "%!PS-Adobe-3.0 EPSF-3.0\nThis must remain literal plain text.",
        "MacCanvas clipboard test\n" * 10_000,
    ]
    for sample in samples:
        encoded = sample.encode("utf-8")
        assert invoke(write, encoded) == b"", "A write must not echo clipboard contents"
        assert invoke(read) == encoded, "Native Mac clipboard content changed"
    # The automatic helper is stricter than the manual helper: no other login session.
    auto_source = (Path(__file__).resolve().parent.parent / "src/clipboard/auto.js").read_text(encoding="utf-8")
    auto = "/usr/bin/osascript -l JavaScript -e " + shlex.quote(auto_source)
    def exchange(since=None, text=None, limit=1048576):
        return json.loads(invoke(auto, json.dumps({"since": since, "text": text, "max_bytes": limit}).encode("utf-8")))
    baseline = exchange()
    if baseline["error"] == "desktop_account":
        print("PASS: auto sync rejects the CI SSH account without its own logged-in graphical desktop; positive auto helper round trip unavailable on this runner.")
    else:
        assert baseline["error"] is None and baseline["text"] is None
        sample = "Auto fixture 中文🍎\n    indented();"
        applied = exchange(baseline["revision"], sample)
        assert applied["error"] is None and applied["applied"]
        assert invoke(read) == sample.encode("utf-8")
        invoke(write, "new remote 中文".encode("utf-8"))
        rejected = exchange(applied["revision"], "old Windows text")
        assert rejected["error"] is None and not rejected["applied"]
        changed = exchange(applied["revision"])
        assert changed["text"] == "new remote 中文"
        assert exchange(changed["revision"])["text"] is None
        for text, limit in [("", 1048576), ("too large", 2), ("bad\0text", 1048576)]:
            assert exchange(changed["revision"], text, limit)["error"] == "helper_failed"
        assert invoke(read) == "new remote 中文".encode("utf-8")
        print("PASS: auto helper graphically logged-in account, Unicode, revision conflict, unchanged snapshots, empty/oversized/NUL rejection.")
    invoke(write, b"")
    print(
        "PASS: real macOS AppKit clipboard; Unicode, empty text, literal RTF/EPS, "
        "large text, and no content echoed by writes."
    )


if __name__ == "__main__":
    main()
