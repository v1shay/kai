#!/usr/bin/env python3
"""Track the macOS arrow cursor, reconstruct its pixels, and encode a clean copy."""

from __future__ import annotations

import argparse
import csv
import subprocess
from pathlib import Path

import cv2
import numpy as np


def locate(
    gray: np.ndarray,
    template: np.ndarray,
    variants: list[np.ndarray],
    bounds: tuple[int, int, int, int],
    previous: tuple[int, int] | None,
):
    height, width = template.shape
    frame_height, frame_width = gray.shape

    def variant_match(roi: np.ndarray, offset: tuple[int, int]):
        bx0, by0, bx1, by1 = bounds
        best = (-1.0, (0, 0))
        for variant in variants:
            cropped = variant[by0:by1, bx0:bx1]
            result = cv2.matchTemplate(roi, cropped, cv2.TM_CCOEFF_NORMED)
            _, score, _, point = cv2.minMaxLoc(result)
            candidate = (offset[0] + point[0] - bx0, offset[1] + point[1] - by0)
            if score > best[0]:
                best = (float(score), candidate)
        return best

    if previous is not None:
        px, py = previous
        radius = 140
        x0, x1 = max(0, px - radius), min(frame_width - width, px + radius)
        y0, y1 = max(0, py - radius), min(frame_height - height, py + radius)
        roi = gray[y0 : y1 + height, x0 : x1 + width]
        result = cv2.matchTemplate(roi, template, cv2.TM_CCOEFF_NORMED)
        _, score, _, point = cv2.minMaxLoc(result)
        candidate = (x0 + point[0], y0 + point[1])
        if score >= 0.68:
            return candidate, float(score), "local"

        variant_score, variant_candidate = variant_match(roi, (x0, y0))
        if variant_score >= 0.70:
            return variant_candidate, variant_score, "local-background"

    result = cv2.matchTemplate(gray, template, cv2.TM_CCOEFF_NORMED)
    _, score, _, point = cv2.minMaxLoc(result)
    if score >= 0.57 or previous is None:
        return point, float(score), "global"

    variant_score, variant_candidate = variant_match(gray, (0, 0))
    if variant_score >= 0.70:
        return variant_candidate, variant_score, "global-background"
    return point, float(score), "global"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--template", type=Path, default=Path("/tmp/macos-arrow.png"))
    parser.add_argument("--log", type=Path)
    args = parser.parse_args()

    cursor = cv2.imread(str(args.template), cv2.IMREAD_UNCHANGED)
    if cursor is None or cursor.shape[2] != 4:
        raise SystemExit(f"Unable to read RGBA cursor template: {args.template}")
    template = cv2.cvtColor(cursor[:, :, :3], cv2.COLOR_BGR2GRAY)
    alpha = cursor[:, :, 3]
    alpha_float = alpha[:, :, None].astype(np.float32) / 255.0
    variants = []
    for level in (0, 64, 128, 192, 255):
        background = np.full_like(cursor[:, :, :3], level)
        composite = cursor[:, :, :3] * alpha_float + background * (1.0 - alpha_float)
        variants.append(cv2.cvtColor(composite.astype(np.uint8), cv2.COLOR_BGR2GRAY))
    ys, xs = np.where(alpha > 8)
    bounds = (int(xs.min()), int(ys.min()), int(xs.max() + 1), int(ys.max() + 1))
    cursor_mask = np.where(alpha > 8, 255, 0).astype(np.uint8)
    cursor_mask = cv2.dilate(cursor_mask, np.ones((3, 3), np.uint8), iterations=1)
    cursor_height, cursor_width = template.shape

    capture = cv2.VideoCapture(str(args.source))
    if not capture.isOpened():
        raise SystemExit(f"Unable to open: {args.source}")
    width = int(capture.get(cv2.CAP_PROP_FRAME_WIDTH))
    height = int(capture.get(cv2.CAP_PROP_FRAME_HEIGHT))
    fps = capture.get(cv2.CAP_PROP_FPS)
    output_fps = round(fps)
    total = int(capture.get(cv2.CAP_PROP_FRAME_COUNT))

    args.output.parent.mkdir(parents=True, exist_ok=True)
    command = [
        "/opt/homebrew/bin/ffmpeg", "-hide_banner", "-loglevel", "error", "-y",
        "-f", "rawvideo", "-pix_fmt", "bgr24", "-s:v", f"{width}x{height}",
        "-r", str(output_fps), "-i", "-", "-an",
        "-c:v", "libx265", "-preset", "slow", "-crf", "13",
        "-tag:v", "hvc1", "-pix_fmt", "yuv420p", "-movflags", "+faststart",
        str(args.output),
    ]
    encoder = subprocess.Popen(command, stdin=subprocess.PIPE)
    if encoder.stdin is None:
        raise SystemExit("Unable to open ffmpeg input pipe")

    rows = []
    previous = None
    index = 0
    try:
        while True:
            ok, frame = capture.read()
            if not ok:
                break
            gray = cv2.cvtColor(frame, cv2.COLOR_BGR2GRAY)
            (x, y), score, mode = locate(gray, template, variants, bounds, previous)

            # Two very fast pointer sweeps cross backgrounds that share the
            # cursor's luminance. Fill their three/five-frame tracking gaps from
            # the reliable positions immediately before and after each sweep.
            if 1509 <= index <= 1511:
                amount = (index - 1508) / (1512 - 1508)
                x = round(363 + (617 - 363) * amount)
                y = round(313 + (141 - 313) * amount)
                score, mode = 1.0, "interpolated"
            elif 1620 <= index <= 1624:
                amount = (index - 1619) / (1625 - 1619)
                x = round(597 + (619 - 597) * amount)
                y = round(630 + (669 - 630) * amount)
                score, mode = 1.0, "interpolated"

            # A confidence this low means the pointer is hidden or partly offscreen.
            # Preserve the source frame instead of removing a false match.
            removed = score >= 0.57
            if removed:
                mask = np.zeros((height, width), np.uint8)
                target_x1, target_y1 = max(0, x), max(0, y)
                target_x2 = min(width, x + cursor_width)
                target_y2 = min(height, y + cursor_height)
                source_x1, source_y1 = target_x1 - x, target_y1 - y
                source_x2 = source_x1 + target_x2 - target_x1
                source_y2 = source_y1 + target_y2 - target_y1
                mask[target_y1:target_y2, target_x1:target_x2] = cursor_mask[
                    source_y1:source_y2, source_x1:source_x2
                ]
                frame = cv2.inpaint(frame, mask, 3, cv2.INPAINT_TELEA)
                previous = (x, y)
            else:
                # During this short section the pointer is partly clipped by the
                # top-left screen edge, so a full cursor template cannot match it.
                if 79 <= index <= 98:
                    mask = np.zeros((height, width), np.uint8)
                    mask[:58, :52] = 255
                    frame = cv2.inpaint(frame, mask, 3, cv2.INPAINT_TELEA)
                    mode = "edge-clipped"
                    removed = True
                else:
                    previous = None

            encoder.stdin.write(frame.tobytes())
            rows.append((index, f"{index / output_fps:.6f}", x, y, f"{score:.6f}", mode, int(removed)))
            index += 1
            if index % 300 == 0:
                print(f"processed {index}/{total}", flush=True)
    finally:
        capture.release()
        encoder.stdin.close()
        status = encoder.wait()
    if status != 0:
        raise SystemExit(f"ffmpeg exited with status {status}")

    if args.log:
        with args.log.open("w", newline="") as handle:
            writer = csv.writer(handle)
            writer.writerow(("frame", "seconds", "x", "y", "score", "search", "removed"))
            writer.writerows(rows)

    scores = np.array([float(row[4]) for row in rows])
    print(
        f"wrote {args.output} | frames={index} | "
        f"score min/median/max={scores.min():.3f}/{np.median(scores):.3f}/{scores.max():.3f}",
        flush=True,
    )


if __name__ == "__main__":
    main()
