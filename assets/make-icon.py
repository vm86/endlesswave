import math

def superellipse(cx, cy, a, n=5.0, steps=360):
    pts = []
    for i in range(steps):
        t = 2 * math.pi * i / steps
        c, s = math.cos(t), math.sin(t)
        x = cx + a * math.copysign(abs(c) ** (2 / n), c)
        y = cy + a * math.copysign(abs(s) ** (2 / n), s)
        pts.append(f"{x:.1f},{y:.1f}")
    return "M" + " L".join(pts) + " Z"

def arc(cx, cy, r, a0, a1):
    """An arc from angle a0 to a1 (degrees, 0 = to the right, y down)."""
    p0 = (cx + r * math.cos(math.radians(a0)), cy + r * math.sin(math.radians(a0)))
    p1 = (cx + r * math.cos(math.radians(a1)), cy + r * math.sin(math.radians(a1)))
    sweep = 1 if a1 > a0 else 0
    return f"M{p0[0]:.1f},{p0[1]:.1f} A{r},{r} 0 0 {sweep} {p1[0]:.1f},{p1[1]:.1f}"

cx = cy = 512
waves = []
for r, op, w in [(235, 0.95, 34), (310, 0.62, 34), (385, 0.34, 34)]:
    waves.append(f'<path d="{arc(cx, cy, r, -38, 38)}" opacity="{op}"/>')           # right side
    waves.append(f'<path d="{arc(cx, cy, r, 180 - 38, 180 + 38)}" opacity="{op}"/>')  # left side

# the play triangle, centered on its centroid, a little to the right (optical centering)
tx, ty, s = cx + 14, cy, 92
tri = f"M{tx - s*0.62:.1f},{ty - s:.1f} L{tx + s*1.0:.1f},{ty:.1f} L{tx - s*0.62:.1f},{ty + s:.1f} Z"

svg = f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024" width="1024" height="1024">
  <defs>
    <linearGradient id="bg" x1="0.15" y1="0" x2="0.85" y2="1">
      <stop offset="0" stop-color="#221a74"/>
      <stop offset="0.55" stop-color="#2b5bff"/>
      <stop offset="1" stop-color="#19b4ff"/>
    </linearGradient>
    <radialGradient id="glow" cx="0.5" cy="0.5" r="0.55">
      <stop offset="0" stop-color="#ffffff" stop-opacity="0.14"/>
      <stop offset="1" stop-color="#ffffff" stop-opacity="0"/>
    </radialGradient>
    <linearGradient id="sheen" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#ffffff" stop-opacity="0.22"/>
      <stop offset="0.5" stop-color="#ffffff" stop-opacity="0"/>
    </linearGradient>
    <linearGradient id="disk" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#ffffff"/>
      <stop offset="1" stop-color="#d6e3ff"/>
    </linearGradient>
    <linearGradient id="play" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="#3b3fe0"/>
      <stop offset="1" stop-color="#1a7bff"/>
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
    <g fill="none" stroke="#ffffff" stroke-width="34" stroke-linecap="round">
      {chr(10).join("      " + w for w in waves).strip()}
    </g>
  </g>
  <circle cx="{cx}" cy="{cy}" r="150" fill="url(#disk)" filter="url(#soft)"/>
  <path d="{tri}" fill="url(#play)" stroke="url(#play)" stroke-width="34" stroke-linejoin="round"/>
</svg>
'''
open("icon.svg", "w").write(svg)
print("svg written", len(svg))
