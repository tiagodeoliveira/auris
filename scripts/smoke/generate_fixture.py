#!/usr/bin/env python3
"""One-off fixture generator. NOT committed.

Reads the meeting transcript from scripts/smoke/script.txt (the lines
between `=== transcript ===` and `=== keywords ===`), sends it to
Soniox TTS, and writes the resulting audio to scripts/smoke/fixture.wav
in the exact format the smoke harness expects (16 kHz mono S16LE WAV).

Loads SONIOX_API_KEY from kleos/.env (shared deployment env). Override
the env path with --env-file. Override the kleos repo location with
--kleos-dir if it's not at the default.

Usage:
    python3 scripts/smoke/generate_fixture.py
    python3 scripts/smoke/generate_fixture.py --voice Adrian --model tts-rt-v1
    python3 scripts/smoke/generate_fixture.py --env-file ~/path/to/.env

Soniox TTS docs: https://soniox.com/docs/tts/rest-api/generate-speech
"""

import argparse
import json
import os
import sys
import urllib.request
import wave
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
SCRIPT_TXT = SCRIPT_DIR / "script.txt"
FIXTURE_PATH = SCRIPT_DIR / "fixture.wav"
DEFAULT_KLEOS_ENV = Path.home() / "src/github.com/tiagodeoliveira/kleos/.env"

SONIOX_TTS_URL = "https://tts-rt.soniox.com/tts"
DEFAULT_MODEL = "tts-rt-v1"
DEFAULT_VOICE = "Adrian"
SAMPLE_RATE = 16000


def parse_env_file(path: Path) -> dict[str, str]:
    """Minimal dotenv parser — skips comments / blanks / lines without `=`."""
    if not path.exists():
        raise FileNotFoundError(f"env file not found: {path}")
    env: dict[str, str] = {}
    for line in path.read_text().splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#") or "=" not in stripped:
            continue
        key, _, value = stripped.partition("=")
        env[key.strip()] = value.strip()
    return env


def extract_transcript(script_text: str) -> str:
    """Pull the lines between `=== transcript ===` and `=== keywords ===`.
    Strip the two markers; join into a single space-separated string
    suitable for TTS input.
    """
    transcript_marker = "=== transcript ==="
    keywords_marker = "=== keywords ==="
    if transcript_marker not in script_text:
        raise ValueError(f"script.txt missing '{transcript_marker}' marker")
    if keywords_marker not in script_text:
        raise ValueError(f"script.txt missing '{keywords_marker}' marker")
    _, _, after = script_text.partition(transcript_marker)
    transcript_block, _, _ = after.partition(keywords_marker)
    # Collapse internal whitespace runs into single spaces so the TTS
    # sees one coherent paragraph rather than line-wrapped fragments.
    return " ".join(transcript_block.split())


def call_soniox_tts(api_key: str, text: str, voice: str, model: str) -> bytes:
    """POST to Soniox TTS, return raw PCM bytes (s16le, mono, 16 kHz)."""
    payload = {
        "model": model,
        "language": "en",
        "voice": voice,
        "audio_format": "pcm_s16le",
        "sample_rate": SAMPLE_RATE,
        "text": text,
    }
    req = urllib.request.Request(
        SONIOX_TTS_URL,
        data=json.dumps(payload).encode("utf-8"),
        headers={
            "Content-Type": "application/json",
            "Authorization": f"Bearer {api_key}",
        },
        method="POST",
    )
    print(
        f"generate_fixture: POST {SONIOX_TTS_URL} "
        f"(model={model}, voice={voice}, sample_rate={SAMPLE_RATE}, "
        f"text={len(text)} chars)"
    )
    with urllib.request.urlopen(req, timeout=120) as resp:
        if resp.status != 200:
            raise RuntimeError(
                f"Soniox TTS returned HTTP {resp.status}: {resp.read()[:500]!r}"
            )
        return resp.read()


def write_wav(pcm_bytes: bytes, out_path: Path) -> None:
    """Wrap raw PCM s16le bytes in a WAV header — mono, 16 kHz, 16-bit."""
    with wave.open(str(out_path), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)  # 16-bit
        w.setframerate(SAMPLE_RATE)
        w.writeframes(pcm_bytes)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--env-file",
        type=Path,
        default=DEFAULT_KLEOS_ENV,
        help=f"Path to env file containing SONIOX_API_KEY (default: {DEFAULT_KLEOS_ENV})",
    )
    parser.add_argument(
        "--voice",
        default=DEFAULT_VOICE,
        help=f"Soniox TTS voice (default: {DEFAULT_VOICE})",
    )
    parser.add_argument(
        "--model",
        default=DEFAULT_MODEL,
        help=f"Soniox TTS model (default: {DEFAULT_MODEL})",
    )
    args = parser.parse_args()

    try:
        env = parse_env_file(args.env_file)
    except FileNotFoundError as e:
        print(f"generate_fixture: {e}", file=sys.stderr)
        return 1

    api_key = env.get("SONIOX_API_KEY") or os.environ.get("SONIOX_API_KEY")
    if not api_key or api_key.startswith("REPLACE"):
        print(
            f"generate_fixture: SONIOX_API_KEY not found in {args.env_file} "
            f"(or value is a REPLACE-ME placeholder)",
            file=sys.stderr,
        )
        return 1

    script_text = SCRIPT_TXT.read_text()
    transcript = extract_transcript(script_text)
    print(f"generate_fixture: transcript = {len(transcript)} chars")

    pcm_bytes = call_soniox_tts(api_key, transcript, args.voice, args.model)
    duration_s = len(pcm_bytes) / (SAMPLE_RATE * 2)
    print(
        f"generate_fixture: received {len(pcm_bytes)} bytes "
        f"of PCM (~{duration_s:.1f}s of audio)"
    )

    write_wav(pcm_bytes, FIXTURE_PATH)
    print(f"generate_fixture: wrote {FIXTURE_PATH}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
