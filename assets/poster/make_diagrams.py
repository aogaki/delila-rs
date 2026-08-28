#!/usr/bin/env python3
"""Generate the four explanatory diagrams for the DELILA-rs poster.

Each returns SVG markup sized to 1600 units across, which is the width of one
poster column (~251 mm at A0), matching the existing charts. Labels carry the
.fs/.fd/.fm class hooks so a font swap in the page's :root reaches them too;
standalone=True inlines a <style> block instead, for use outside the page.
"""

INK, SOFT, MUTED, RULE = "#111111", "#4A4840", "#8C8A84", "#C9C4B5"
ACCENT, VERIFY, PAPER, PANEL = "#E84E1B", "#1D5E44", "#F2F0EA", "#E8E5D9"

STANDALONE_STYLE = (
    '<style>'
    '.fs{font-family:"IBM Plex Sans","Helvetica Neue",Helvetica,Arial,sans-serif}'
    '.fd{font-family:"Archivo","Helvetica Neue",Helvetica,Arial,sans-serif}'
    '.fm{font-family:"IBM Plex Mono",Menlo,Consolas,monospace}'
    '</style>'
)


def _open(vb, label, standalone):
    """vb is the full viewBox string; standalone files also get xmlns + intrinsic
    size so they open correctly in Affinity and in a bare browser tab."""
    w, h = vb.split()[2:]
    if standalone:
        return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{vb}" '
                f'width="{w}" height="{h}" role="img" aria-label="{label}">'
                + STANDALONE_STYLE)
    return f'<svg viewBox="{vb}" role="img" aria-label="{label}">' 


def tri(x, y, size=18, color=ACCENT):
    """Right-pointing arrowhead with its tip at (x, y)."""
    return (f'<path d="M{x - size},{y - size * 0.62} L{x},{y} '
            f'L{x - size},{y + size * 0.62} Z" fill="{color}"/>')


# ---------------------------------------------------------------- A ---------
def diagram_a(standalone=False):
    """Triggered vs listmode: where the stream narrows, and where it doesn't."""
    o = [_open("0 0 1600 474",
               "A triggered readout narrows the data stream at a hardware trigger and "
               "discards the rest; listmode keeps every hit, so the rate written to disk "
               "equals the rate at the detector",
               standalone)]

    def hits(x0, x1, y0, h, pitch):
        n = int((x1 - x0) / pitch)
        return "".join(
            f'<rect x="{x0 + 6 + i * pitch:.0f}" y="{y0 + 5}" width="3" '
            f'height="{h - 10}" fill="{PAPER}" opacity=".85"/>' for i in range(n))

    # ---- row 1: triggered ----
    o.append(f'<text x="0" y="46" class="fm" font-size="33" fill="{INK}" letter-spacing="3">TRIGGERED</text>')
    o.append(f'<text x="790" y="46" class="fm" font-size="31" fill="{MUTED}" text-anchor="middle">hardware trigger</text>')
    o.append(f'<rect x="300" y="64" width="460" height="84" fill="{ACCENT}"/>')
    o.append(hits(300, 760, 64, 84, 23))
    o.append(f'<rect x="760" y="48" width="60" height="116" fill="{INK}"/>')
    o.append(f'<rect x="820" y="96" width="430" height="20" fill="{ACCENT}"/>')
    o.append(hits(820, 1250, 96, 20, 86))
    o.append(f'<line x1="1250" y1="106" x2="1296" y2="106" stroke="{ACCENT}" stroke-width="5"/>')
    o.append(tri(1310, 106))
    o.append(f'<text x="1330" y="116" class="fm" font-size="31" fill="{SOFT}">DISK</text>')
    # the discarded fraction falls out under the gate
    for i, (dx, dy) in enumerate(((58, 58), (112, 74), (166, 60), (222, 82), (278, 66), (336, 86))):
        o.append(f'<line x1="{790 + dx * 0.35:.0f}" y1="{164 + dy * 0.35:.0f}" '
                 f'x2="{790 + dx:.0f}" y2="{164 + dy:.0f}" stroke="{MUTED}" stroke-width="4"/>')
    o.append(f'<text x="836" y="286" class="fm" font-size="30" fill="{MUTED}">discarded at the front end</text>')

    # ---- row 2: listmode ----
    o.append(f'<text x="0" y="356" class="fm" font-size="33" fill="{INK}" letter-spacing="3">LISTMODE</text>')
    o.append(f'<line x1="790" y1="342" x2="790" y2="458" stroke="{RULE}" stroke-width="4" stroke-dasharray="10 9"/>')
    o.append(f'<text x="790" y="356" class="fm" font-size="31" fill="{MUTED}" text-anchor="middle">no gate</text>')
    o.append(f'<rect x="300" y="374" width="950" height="84" fill="{ACCENT}"/>')
    o.append(hits(300, 1250, 374, 84, 23))
    o.append(f'<line x1="1250" y1="416" x2="1296" y2="416" stroke="{ACCENT}" stroke-width="5"/>')
    o.append(tri(1310, 416))
    o.append(f'<text x="1330" y="426" class="fm" font-size="31" fill="{SOFT}">DISK</text>')
    o.append('</svg>')
    return "\n".join(o)


# ---------------------------------------------------------------- B ---------
def diagram_b(standalone=False):
    """Hits arrive in network order and leave in detector-time order."""
    BOARD = {"A": ACCENT, "B": INK, "C": VERIFY}
    arrive = ["A3", "B1", "A1", "C2", "B2", "A2", "C1"]
    sortd = ["A1", "B1", "C1", "A2", "B2", "C2", "A3"]
    xs = [188 + i * 168 for i in range(7)]
    CW, CH = 122, 50
    o = [_open("0 0 1600 512",
               "Hits arriving in network order are re-ordered by detector time; a safe-horizon "
               "window bounds the look-back and fixed-span chunks are handed to parallel workers",
               standalone)]

    o.append(f'<text x="0" y="30" class="fm" font-size="30" fill="{MUTED}" letter-spacing="3">ARRIVAL ORDER · NETWORK</text>')
    for x, tag in zip(xs, arrive):
        c = BOARD[tag[0]]
        o.append(f'<rect x="{x}" y="48" width="{CW}" height="{CH}" fill="{c}"/>')
        o.append(f'<text x="{x + CW / 2:.0f}" y="{48 + 35}" class="fm" font-size="39" font-weight="600" '
                 f'fill="{PAPER}" text-anchor="middle">{tag}</text>')
    # crossing connectors
    for i, tag in enumerate(arrive):
        j = sortd.index(tag)
        x0, x1 = xs[i] + CW / 2, xs[j] + CW / 2
        o.append(f'<path d="M{x0:.0f},98 C{x0:.0f},158 {x1:.0f},158 {x1:.0f},218" '
                 f'fill="none" stroke="{RULE}" stroke-width="3"/>')
    for x, tag in zip(xs, sortd):
        c = BOARD[tag[0]]
        o.append(f'<rect x="{x}" y="218" width="{CW}" height="{CH}" fill="{c}"/>')
        o.append(f'<text x="{x + CW / 2:.0f}" y="{218 + 35}" class="fm" font-size="39" font-weight="600" '
                 f'fill="{PAPER}" text-anchor="middle">{tag}</text>')
    # detector-time axis — label sits after the arrow so it cannot collide
    o.append(f'<line x1="150" y1="300" x2="1250" y2="300" stroke="{INK}" stroke-width="4"/>')
    o.append(tri(1266, 300, 16, INK))
    o.append(f'<text x="1288" y="309" class="fm" font-size="30" fill="{INK}" letter-spacing="2">TIME</text>')
    o.append(f'<text x="0" y="309" class="fm" font-size="30" fill="{MUTED}" letter-spacing="3">DETECTOR</text>')
    # safe horizon — its own band
    o.append(f'<path d="M188,326 L188,352 L692,352 L692,326" fill="none" stroke="{MUTED}" stroke-width="3"/>')
    o.append(f'<text x="440" y="390" class="fm" font-size="31" fill="{MUTED}" text-anchor="middle">safe horizon 50 ms</text>')
    # chunk spans — a separate band below the horizon
    for x0, x1, who in ((188, 692, "worker 1"), (716, 1250, "worker 2")):
        o.append(f'<path d="M{x0},424 L{x0},450 L{x1},450 L{x1},424" fill="none" stroke="{ACCENT}" stroke-width="4"/>')
        o.append(f'<text x="{(x0 + x1) / 2:.0f}" y="488" class="fm" font-size="31" fill="{ACCENT}" '
                 f'text-anchor="middle">chunk 100 ms · {who}</text>')
    o.append('</svg>')
    return "\n".join(o)


# ---------------------------------------------------------------- C ---------
def diagram_c(standalone=False):
    """.delila layout: embedded schema, independent blocks, survivable tail."""
    o = [_open("0 0 1600 250",
               "A delila file begins with its own schema definition, followed by independently "
               "readable blocks, so a file cut short still yields every completed block",
               standalone)]
    y, h = 56, 86
    o.append(f'<rect x="0" y="{y}" width="330" height="{h}" fill="{INK}"/>')
    o.append(f'<text x="165" y="{y + 55}" class="fm" font-size="38" font-weight="600" fill="{PAPER}" '
             f'text-anchor="middle">SCHEMA</text>')
    for i in range(4):
        x = 346 + i * 226
        o.append(f'<rect x="{x}" y="{y}" width="210" height="{h}" fill="{PANEL}" stroke="{INK}" stroke-width="4"/>')
        o.append(f'<text x="{x + 105}" y="{y + 55}" class="fm" font-size="38" fill="{INK}" '
                 f'text-anchor="middle">block {i}</text>')
    # torn tail
    o.append(f'<path d="M1250,{y} L1290,{y} L1268,{y + 22} L1300,{y + 44} L1262,{y + 64} L1292,{y + h} L1250,{y + h} Z" '
             f'fill="{PANEL}" stroke="{ACCENT}" stroke-width="4" stroke-dasharray="9 7"/>')
    o.append(f'<text x="1316" y="{y + 42}" class="fm" font-size="32" fill="{ACCENT}">killed</text>')
    o.append(f'<text x="1316" y="{y + 76}" class="fm" font-size="32" fill="{ACCENT}">here</text>')
    o.append(f'<path d="M0,{y + h + 26} L0,{y + h + 44} L1240,{y + h + 44} L1240,{y + h + 26}" '
             f'fill="none" stroke="{VERIFY}" stroke-width="4"/>')
    o.append(f'<text x="620" y="{y + h + 82}" class="fm" font-size="32" fill="{VERIFY}" '
             f'text-anchor="middle">still recovered · this is how the 15 killed runs were re-read</text>')
    o.append(f'<text x="0" y="30" class="fm" font-size="30" fill="{MUTED}" letter-spacing="3">.DELILA · SELF-DESCRIBING</text>')
    o.append('</svg>')
    return "\n".join(o)


# ---------------------------------------------------------------- D ---------
def diagram_d(standalone=False):
    """ELIADE: eight hosts on a common clock, RUN daisy-chained."""
    o = [_open("0 0 1600 384",
               "Eight ELIADE hosts share a common clock while the run signal is daisy-chained "
               "from host to host, adding about eight nanoseconds of jitter per hop; each host "
               "reads four V1725 and one V1730",
               standalone)]
    xs = [96, 480, 864]
    BW, BH = 300, 150
    top = 132
    # clock bus
    o.append(f'<line x1="0" y1="52" x2="1420" y2="52" stroke="{INK}" stroke-width="5"/>')
    o.append(f'<text x="1436" y="62" class="fm" font-size="31" fill="{INK}">CLOCK</text>')
    for x in xs:
        o.append(f'<line x1="{x + BW / 2:.0f}" y1="52" x2="{x + BW / 2:.0f}" y2="{top}" stroke="{INK}" stroke-width="4"/>')
    # RUN daisy chain
    o.append(f'<text x="1436" y="116" class="fm" font-size="31" fill="{ACCENT}">RUN</text>')
    o.append(f'<line x1="0" y1="106" x2="{xs[0] + BW / 2:.0f}" y2="106" stroke="{ACCENT}" stroke-width="5"/>')
    for a, b in zip(xs, xs[1:] + [1248]):
        o.append(f'<line x1="{a + BW / 2:.0f}" y1="106" x2="{b + BW / 2:.0f}" y2="106" stroke="{ACCENT}" stroke-width="5"/>')
        o.append(tri(b + BW / 2 - 6, 106, 15))
        o.append(f'<text x="{(a + b) / 2 + BW / 2:.0f}" y="92" class="fm" font-size="30" fill="{ACCENT}" '
                 f'text-anchor="middle">≈8 ns</text>')
    for x in xs:
        o.append(f'<rect x="{x}" y="{top}" width="{BW}" height="{BH}" fill="{PANEL}" stroke="{INK}" stroke-width="5"/>')
        o.append(f'<text x="{x + BW / 2:.0f}" y="{top + 46}" class="fd" font-size="39" font-weight="700" '
                 f'fill="{INK}" text-anchor="middle">HOST</text>')
        o.append(f'<text x="{x + BW / 2:.0f}" y="{top + 88}" class="fm" font-size="32" fill="{SOFT}" '
                 f'text-anchor="middle">4× V1725 PHA</text>')
        o.append(f'<text x="{x + BW / 2:.0f}" y="{top + 124}" class="fm" font-size="32" fill="{SOFT}" '
                 f'text-anchor="middle">1× V1730 PSD</text>')
        o.append(f'<text x="{x + BW / 2:.0f}" y="{top + BH + 44}" class="fm" font-size="31" fill="{MUTED}" '
                 f'text-anchor="middle">clover HPGe</text>')
    o.append(f'<text x="1290" y="{top + 96}" class="fm" font-size="70" fill="{MUTED}" '
             f'text-anchor="middle" letter-spacing="8">···</text>')
    o.append(f'<text x="1290" y="{top + BH + 44}" class="fm" font-size="31" fill="{MUTED}" '
             f'text-anchor="middle">×8 hosts</text>')
    o.append(f'<text x="0" y="30" class="fm" font-size="30" fill="{MUTED}" letter-spacing="3">CLOCK &amp; RUN DISTRIBUTION</text>')
    o.append('</svg>')
    return "\n".join(o)


DIAGRAMS = {
    "diagram_problem_funnel": diagram_a,
    "diagram_event_building": diagram_b,
    "diagram_delila_format": diagram_c,
    "diagram_eliade_topology": diagram_d,
}

if __name__ == "__main__":
    for name, fn in DIAGRAMS.items():
        open(f"{name}.svg", "w").write(fn(standalone=True) + "\n")
        print("wrote", f"{name}.svg")
