#!/usr/bin/env python3
"""Generate the self-contained ModelTrace scoring golden fixture.

Regeneration from the repository root:

    git clone https://github.com/xqy2006/ModelTrace.git ../ModelTrace
    git -C ../ModelTrace checkout bff6cb1ff6f91e70a4e61d3f106f8c4925918eea
    python3 backend/crates/model-fingerprint/tools/generate_golden.py \
        --modeltrace ../ModelTrace

The script reads `codex-plugin/modeltrace-guard/assets/unified_bank.json`,
`data/gpt_reference.jsonl`, and `data/claude_reference.jsonl` from that pinned
checkout. It deliberately uses only the Python standard library: no NumPy or
other ML dependency is required. The generated fixture contains the input text
and fixed outputs, so tests never read the upstream checkout at runtime.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import subprocess
from collections import Counter
from pathlib import Path
from typing import Iterable

SOURCE_COMMIT = "bff6cb1ff6f91e70a4e61d3f106f8c4925918eea"
BANK_SHA256 = "5886b3afc6302abee7fe3637a6578c39b9f6d2932cbc9029f6c1c4c6f398bca6"
VALUE_MIN = 1
VALUE_MAX = 355
DIMENSION = VALUE_MAX - VALUE_MIN + 1
ORDERED_DIMENSION = 74
ALPHA = 0.5
MINIMUM_SCALE = 1e-12
WEIGHTS = (0.0, 0.25)


def parse_numbers(text: str) -> list[int]:
    runs: list[list[int]] = []
    current: list[int] = []
    previous_end = 0
    for match in re.finditer(r"\d+", text):
        separator = text[previous_end : match.start()]
        value = int(match.group())
        if current and any(character.isalpha() for character in separator):
            runs.append(current)
            current = []
        if VALUE_MIN <= value <= VALUE_MAX:
            current.append(value)
        previous_end = match.end()
    if current:
        runs.append(current)
    return max(runs, key=len) if runs else []


def standardize(values: list[float]) -> list[float]:
    mean = sum(values) / len(values)
    variance = sum((value - mean) ** 2 for value in values) / len(values)
    scale = max(math.sqrt(variance), MINIMUM_SCALE)
    return [(value - mean) / scale for value in values]


def dot(left: Iterable[float], right: Iterable[float]) -> float:
    return sum(a * b for a, b in zip(left, right))


def normalize(values: list[float]) -> list[float]:
    scale = max(math.sqrt(dot(values, values)), MINIMUM_SCALE)
    return [value / scale for value in values]


def project_out(values: list[float], basis: list[list[float]]) -> list[float]:
    # Matches `(v @ B.T) @ B`: every coefficient is calculated from original v.
    coefficients = [dot(values, vector) for vector in basis]
    return [
        value
        - sum(coefficient * vector[index] for coefficient, vector in zip(coefficients, basis))
        for index, value in enumerate(values)
    ]


def hellinger_feature(numbers: list[int]) -> list[float]:
    counts = [0] * DIMENSION
    for number in numbers:
        counts[number - VALUE_MIN] += 1
    total = sum(counts) + ALPHA * DIMENSION
    return [math.sqrt((count + ALPHA) / total) for count in counts]


def marginal_scores(numbers: list[int], bank: dict) -> list[float]:
    artifact = bank["robust"]["hellinger"]
    feature = hellinger_feature(numbers)
    standardized = [
        (value - mean) / scale
        for value, mean, scale in zip(
            feature, artifact["feature_mean"], artifact["feature_scale"]
        )
    ]
    projected = normalize(project_out(standardized, artifact["nuisance_basis"]))
    nuisance = standardize(
        [dot(projected, centroid) for centroid in artifact["centroids"]]
    )
    # fingerprint.py:86 and :88 intentionally standardize twice.
    return standardize(nuisance)


def ordered_block_feature(numbers: list[int]) -> list[float]:
    base, remainder = divmod(len(numbers), 4)
    pieces: list[float] = []
    start = 0
    for chunk_index in range(4):
        size = base + (1 if chunk_index < remainder else 0)
        bins = [0] * 16
        for number in numbers[start : start + size]:
            index = min(15, ((number - 1) * 16) // DIMENSION)
            bins[index] += 1
        start += size
        total = size + ALPHA * len(bins)
        pieces.extend(math.sqrt((count + ALPHA) / total) for count in bins)

    last_digits = [0] * 10
    for number in numbers:
        last_digits[number % 10] += 1
    total = len(numbers) + ALPHA * len(last_digits)
    pieces.extend(math.sqrt((count + ALPHA) / total) for count in last_digits)
    assert len(pieces) == ORDERED_DIMENSION
    return pieces


def ordered_scores(numbers: list[int], bank: dict) -> list[float]:
    artifact = bank["robust"]["ordered_blocks"]
    feature = ordered_block_feature(numbers)
    standardized = [
        (value - mean) / scale
        for value, mean, scale in zip(
            feature, artifact["feature_mean"], artifact["feature_scale"]
        )
    ]

    unit = normalize(standardized)
    environment_scores = [
        [dot(unit, centroid) for centroid in environment]
        for environment in artifact["environment_centroids"]
    ]
    template = standardize(
        [
            max(environment[model_index] for environment in environment_scores)
            for model_index in range(len(artifact["centroids"]))
        ]
    )

    projected = normalize(project_out(standardized, artifact["nuisance_basis"]))
    nuisance = standardize(
        [dot(projected, centroid) for centroid in artifact["centroids"]]
    )
    return standardize(
        [
            0.5 * template_score + 0.5 * nuisance_score
            for template_score, nuisance_score in zip(template, nuisance)
        ]
    )


def score(numbers: list[int], bank: dict, weight: float) -> list[float]:
    marginal = marginal_scores(numbers, bank)
    if weight == 0.0:
        return marginal
    ordered = ordered_scores(numbers, bank)
    return [
        (1.0 - weight) * marginal_score + weight * ordered_score
        for marginal_score, ordered_score in zip(marginal, ordered)
    ]


def load_rows(modeltrace: Path) -> list[dict]:
    rows = []
    for relative in ("data/gpt_reference.jsonl", "data/claude_reference.jsonl"):
        with (modeltrace / relative).open(encoding="utf-8") as source:
            rows.extend(json.loads(line) for line in source if line.strip())
    if len(rows) != 468:
        raise ValueError(f"expected 468 reference rows, got {len(rows)}")
    counts = Counter(row["model_id"] for row in rows)
    if len(counts) != 13 or set(counts.values()) != {36}:
        raise ValueError(f"expected 13 models with 36 rows each, got {counts}")
    return rows


def verify_source(modeltrace: Path, bank_path: Path) -> None:
    commit = subprocess.check_output(
        ["git", "-C", str(modeltrace), "rev-parse", "HEAD"], text=True
    ).strip()
    if commit != SOURCE_COMMIT:
        raise ValueError(f"ModelTrace must be pinned to {SOURCE_COMMIT}, got {commit}")
    digest = hashlib.sha256(bank_path.read_bytes()).hexdigest()
    if digest != BANK_SHA256:
        raise ValueError(f"unexpected bank sha256: {digest}")


def build_fixture(modeltrace: Path) -> dict:
    bank_path = modeltrace / "codex-plugin/modeltrace-guard/assets/unified_bank.json"
    verify_source(modeltrace, bank_path)
    bank = json.loads(bank_path.read_text(encoding="utf-8"))
    model_order = bank["robust"]["model_order"]
    if model_order != [model["id"] for model in bank["models"]]:
        raise ValueError("bank model order does not match models[].id")

    cases = []
    for row in load_rows(modeltrace):
        claimed_model = row["model_id"]
        if claimed_model not in model_order:
            raise ValueError(f"reference model missing from bank: {claimed_model}")
        claimed_index = model_order.index(claimed_model)
        numbers = parse_numbers(row["text"])
        weights = []
        for weight in WEIGHTS:
            scores = score(numbers, bank, weight)
            rank_one_index = max(range(len(scores)), key=scores.__getitem__)
            max_other = max(
                value for index, value in enumerate(scores) if index != claimed_index
            )
            weights.append(
                {
                    "ordered_block_weight": weight,
                    "scores": scores,
                    "rank_one": model_order[rank_one_index],
                    "margin": scores[claimed_index] - max_other,
                }
            )
        cases.append(
            {
                "id": row["row_id"],
                "claimed_model": claimed_model,
                "text": row["text"],
                "weights": weights,
            }
        )

    return {
        "source_commit": SOURCE_COMMIT,
        "bank_sha256": BANK_SHA256,
        "model_order": model_order,
        "cases": cases,
    }


def main() -> None:
    crate_root = Path(__file__).resolve().parent.parent
    parser = argparse.ArgumentParser()
    parser.add_argument("--modeltrace", type=Path, required=True)
    parser.add_argument(
        "--output",
        type=Path,
        default=crate_root / "tests/fixtures/modeltrace-golden-20260912.json",
    )
    args = parser.parse_args()

    fixture = build_fixture(args.modeltrace.resolve())
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(fixture, ensure_ascii=False, separators=(",", ":")) + "\n",
        encoding="utf-8",
    )
    print(f"wrote {len(fixture['cases'])} cases to {args.output}")


if __name__ == "__main__":
    main()
