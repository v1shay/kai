#!/usr/bin/env python3
"""Build deterministic UI gradient tokens from every pet's neutral sprite frame."""
import colorsys, json, math, re, subprocess
from pathlib import Path

ROOT = Path(__file__).parent
CATALOG = json.loads((ROOT / "animation_catalog.json").read_text())

def rgb(hex_color): return tuple(int(hex_color[i:i+2], 16) for i in (1, 3, 5))
def hexc(c): return "#" + "".join(f"{max(0,min(255,round(v))):02X}" for v in c)
def mix(a, b, t): return hexc(tuple(x*(1-t)+y*t for x, y in zip(rgb(a), rgb(b))))
def lum(c):
    v = [x/255 for x in c]
    v = [x/12.92 if x <= .04045 else ((x+.055)/1.055)**2.4 for x in v]
    return .2126*v[0] + .7152*v[1] + .0722*v[2]
def distance(a, b): return math.sqrt(sum((x-y)**2 for x, y in zip(a, b))) / 441.7
def stop(location, color): return {"location": location, "color": color}
def gradient(angle, duration, colors):
    last = len(colors)-1
    return {"angleDegrees": angle, "cycleDurationMs": duration,
            "stops": [stop(round(i/last, 3), c) for i, c in enumerate(colors)]}

def palette(pet):
    frame = pet["neutralFrame"]; grid = pet["grid"]
    crop = f'{grid["frameWidth"]}x{grid["frameHeight"]}+{frame["column"]*grid["frameWidth"]}+{frame["row"]*grid["frameHeight"]}'
    command = ["magick", str(ROOT/pet["spritesheet"]), "-crop", crop, "+repage",
               "-alpha", "on", "-colors", "16", "-format", "%c", "histogram:info:-"]
    lines = subprocess.check_output(command, text=True).splitlines()
    colors = []
    for line in lines:
        match = re.search(r"^\s*(\d+): \(([\d.]+),([\d.]+),([\d.]+),([\d.]+)\)", line)
        if not match or float(match.group(5)) < 220: continue
        count = int(match.group(1)); color = tuple(float(match.group(i)) for i in range(2, 5))
        h, l, s = colorsys.rgb_to_hls(*(x/255 for x in color)); chroma = (max(color)-min(color))/255
        colors.append({"count": count, "rgb": color, "hex": hexc(color), "l": l, "s": s, "chroma": chroma})
    total = sum(c["count"] for c in colors); maximum = max(c["count"] for c in colors)
    viable = [c for c in colors if c["count"] >= max(8, total*.001)] or colors
    primary = max(viable, key=lambda c: c["count"]/maximum*.50 + c["chroma"]*.40 - abs(c["l"]-.55)*.18)
    distinct = [c for c in viable if distance(c["rgb"], primary["rgb"]) >= .10] or viable
    accent = max(distinct, key=lambda c: c["chroma"]*.72 - abs(c["l"]-.58)*.15 + c["count"]/maximum*.13)
    secondary = max(viable, key=lambda c: distance(c["rgb"], primary["rgb"])*.48 + c["chroma"]*.35 + c["count"]/maximum*.17)
    shadow = min(viable, key=lambda c: c["l"])
    highlight = max(viable, key=lambda c: c["l"] + c["s"]*.08)
    return {"shadow": shadow["hex"], "primary": primary["hex"], "secondary": secondary["hex"],
            "accent": accent["hex"], "highlight": highlight["hex"]}

profiles = {}
for pet in CATALOG["pets"]:
    p = palette(pet); avg = tuple(sum(rgb(p[k])[i] for k in ("primary", "accent"))/2 for i in range(3))
    foreground = "#000000" if lum(avg) > .45 else "#FFFFFF"
    profiles[pet["id"]] = {
        "displayName": pet["displayName"], "palette": {**p, "foreground": foreground},
        "gradients": {
            "ambient": gradient(90, 3200, [p["shadow"], p["primary"], p["accent"]]),
            "thinking": gradient(115, 1500, [p["primary"], p["accent"], p["highlight"], p["primary"]]),
            "working": gradient(0, 1100, [p["shadow"], p["primary"], p["secondary"], p["accent"]]),
            "success": gradient(25, 900, [p["shadow"], p["accent"], p["highlight"]]),
            "warning": gradient(25, 900, [p["shadow"], p["secondary"], p["accent"], p["highlight"]]),
            "error": gradient(25, 900, [p["shadow"], p["secondary"], p["accent"]])
        }
    }

output = {"formatVersion": 1, "colorSpace": "sRGB", "profileByPetID": profiles}
(ROOT / "gradient_profiles.json").write_text(json.dumps(output, ensure_ascii=False, indent=2) + "\n")
print(f'Wrote {len(profiles)} profiles to {ROOT/"gradient_profiles.json"}')
