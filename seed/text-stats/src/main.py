"""Paste a line of text, get characters, words, unique words, and reading time."""
import re
import sys

for line in sys.stdin:
    text = line.rstrip("\n")
    words = re.findall(r"[\w'-]+", text)
    sentences = [s for s in re.split(r"[.!?]+", text) if s.strip()]
    seconds = round(len(words) / 3.5, 1)  # ~210 wpm
    print(
        f"chars={len(text)} words={len(words)} unique={len({w.lower() for w in words})} "
        f"sentences={len(sentences)} reading~{seconds}s",
        flush=True,
    )
