"""Draws icon.svg (the app icon): two notes whose beam is a wave that does not end.

    python3 make-icon.py        # then render icon.svg to icon.png (1024 px) and run `npx tauri icon`

The menu bar icon (a play button with radio waves, a template image) is drawn by hand in tray.svg;
src-tauri/icons/tray.png is made from it.
"""
import math
from pathlib import Path


def superellipse(cx, cy, a, n=5.0, steps=360):
    """The rounded square of macOS icons."""
    pts = []
    for i in range(steps):
        t = 2 * math.pi * i / steps
        c, s = math.cos(t), math.sin(t)
        x = cx + a * math.copysign(abs(c) ** (2 / n), c)
        y = cy + a * math.copysign(abs(s) ** (2 / n), s)
        pts.append(f"{x:.1f},{y:.1f}")
    return "M" + " L".join(pts) + " Z"


def infinity(cx, cy, a, squash=1.0, steps=240):
    """The lemniscate of Gerono-Bernoulli: one closed line, two loops. `a` is the half width."""
    pts = []
    for i in range(steps):
        t = 2 * math.pi * i / steps
        d = 1 + math.sin(t) ** 2
        pts.append((cx + a * math.cos(t) / d, cy + squash * a * math.sin(t) * math.cos(t) / d))
    return pts


def path(pts):
    return "M" + " L".join(f"{x:.1f},{y:.1f}" for x, y in pts) + " Z"


def wave(cx, cy, a, amplitude, cycles, steps=400):
    """A sine wave from the left tip to the right tip of the infinity sign."""
    pts = []
    for i in range(steps + 1):
        u = i / steps
        x = cx - a + 2 * a * u
        envelope = math.sin(math.pi * u) ** 0.8
        pts.append((x, cy + amplitude * envelope * math.sin(2 * math.pi * cycles * u)))
    return "M" + " L".join(f"{x:.1f},{y:.1f}" for x, y in pts)


cx = cy = 512

# Two notes joined by a beam that is a wave: music, and a wave that does not end; it runs on past
# the notes to both edges and fades out.
HEADS = [(352, 640), (640, 590)]
RX, RY, TILT = 92, 66, -22
STEM_W = 32
STEM_TOP = [300, 246]
heads = "\n".join(
    f'    <ellipse cx="{x}" cy="{y}" rx="{RX}" ry="{RY}" transform="rotate({TILT} {x} {y})"/>' for x, y in HEADS
)
stems = "\n".join(
    f'    <line x1="{x + RX - 18}" y1="{y - 24}" x2="{x + RX - 18}" y2="{top}" stroke-width="{STEM_W}" stroke-linecap="butt"/>'
    for (x, y), top in zip(HEADS, STEM_TOP)
)
sx = [HEADS[0][0] + RX - 18, HEADS[1][0] + RX - 18]
beam = f"M{sx[0]},{STEM_TOP[0]} C{sx[0] + 70},{STEM_TOP[0] - 70} {sx[1] - 90},{STEM_TOP[1] + 80} {sx[1]},{STEM_TOP[1]}"
left = f"M{sx[0]},{STEM_TOP[0]} C{sx[0] - 70},{STEM_TOP[0] + 80} {sx[0] - 190},{STEM_TOP[0] + 50} {sx[0] - 330},{STEM_TOP[0] - 6}"
right = f"M{sx[1]},{STEM_TOP[1]} C{sx[1] + 70},{STEM_TOP[1] - 80} {sx[1] + 160},{STEM_TOP[1] - 60} {sx[1] + 260},{STEM_TOP[1] + 6}"
def sea(y, amp, phase, cycles=1.5):
    pts = []
    for i in range(0, 61):
        u = i / 60
        pts.append((60 + 904 * u, y + amp * math.sin(2 * math.pi * cycles * u + phase)))
    return "M" + " L".join(f"{x:.1f},{yy:.1f}" for x, yy in pts)


ribbons = "\n".join(
    f'    <path d="{sea(y, amp, ph)}" stroke-width="{w}" opacity="{op}"/>'
    for y, amp, ph, w, op in [(792, 34, 0.0, 26, 0.55), (846, 30, 1.2, 22, 0.34), (894, 26, 2.4, 18, 0.2)]
)
loops = f"""{ribbons}
    <path d="{left}" stroke="url(#fadeL)" stroke-width="40"/>
    <path d="{right}" stroke="url(#fadeR)" stroke-width="40"/>
    <path d="{beam}" stroke-width="58"/>
{stems}"""

svg = f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024">
  <defs>
    <linearGradient id="bg" x1="0.15" y1="0" x2="0.85" y2="1">
      <stop offset="0" stop-color="#1b1466"/>
      <stop offset="0.55" stop-color="#2b5bff"/>
      <stop offset="1" stop-color="#19c2ff"/>
    </linearGradient>
    <radialGradient id="glow" cx="0.5" cy="0.5" r="0.55">
      <stop offset="0" stop-color="#ffffff" stop-opacity="0.16"/>
      <stop offset="1" stop-color="#ffffff" stop-opacity="0"/>
    </radialGradient>
    <linearGradient id="sheen" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#ffffff" stop-opacity="0.22"/>
      <stop offset="0.5" stop-color="#ffffff" stop-opacity="0"/>
    </linearGradient>
    <linearGradient id="ink" gradientUnits="userSpaceOnUse" x1="170" y1="0" x2="854" y2="0">
      <stop offset="0" stop-color="#ffffff"/>
      <stop offset="1" stop-color="#c9ecff"/>
    </linearGradient>
    <linearGradient id="fadeL" gradientUnits="userSpaceOnUse" x1="{sx[0]}" y1="0" x2="{sx[0] - 330}" y2="0">
      <stop offset="0" stop-color="#ffffff" stop-opacity="0.9"/>
      <stop offset="1" stop-color="#ffffff" stop-opacity="0"/>
    </linearGradient>
    <linearGradient id="fadeR" gradientUnits="userSpaceOnUse" x1="{sx[1]}" y1="0" x2="{sx[1] + 260}" y2="0">
      <stop offset="0" stop-color="#ffffff" stop-opacity="0.9"/>
      <stop offset="1" stop-color="#ffffff" stop-opacity="0"/>
    </linearGradient>
    <filter id="shadow" x="-20%" y="-20%" width="140%" height="150%">
      <feDropShadow dx="0" dy="14" stdDeviation="16" flood-color="#0b1030" flood-opacity="0.45"/>
    </filter>
    <filter id="soft" x="-30%" y="-30%" width="160%" height="170%">
      <feDropShadow dx="0" dy="8" stdDeviation="10" flood-color="#0b1030" flood-opacity="0.35"/>
    </filter>
    <clipPath id="clip"><path d="{superellipse(512, 512, 412)}"/></clipPath>
  </defs>

  <path d="{superellipse(512, 512, 412)}" fill="url(#bg)" filter="url(#shadow)"/>
  <g clip-path="url(#clip)">
    <rect width="1024" height="1024" fill="url(#glow)"/>
    <rect width="1024" height="1024" fill="url(#sheen)"/>
  </g>
  <g clip-path="url(#clip)"><g fill="none" stroke="#ffffff" stroke-linecap="round" stroke-linejoin="round" filter="url(#soft)">
{loops}
  </g></g>
  <g fill="#ffffff" filter="url(#soft)">
{heads}
  </g>
</svg>
'''
(Path(__file__).parent / "icon.svg").write_text(svg)

print("written icon.svg")
