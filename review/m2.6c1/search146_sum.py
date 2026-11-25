"""Summarises the livelock search's three runs (search146.py with
rec146s.rs at 0213a6c, 4ff4549 and the base a4e004e) into one file:
python3 -I search146_sum.py <head.out> <4ff4549.out> <base.out> > search146.out

- each head's tally of how the runs ended;
- the livelocks at the head against 4ff4549 and the base;
- the livelocks left at the head, by class, with whether skyzfsd can
  answer so (the filter below), one trail each;
- every livelock left at the head (its script, without the trail)."""
import re, sys
from collections import Counter, defaultdict

def read(path):
    lines = open(path).read().splitlines()
    tally = [l for l in lines if l.startswith("R7 ")]
    ll = {}
    for l in lines:
        m = re.match(r"LIVELOCK (publishes \d old \w+) R7Script \{ first: ([^,]+), later: ([^,]+), fate: ([^,]+), look: (\w+) \}: (.*)", l)
        if m:
            ll[m.groups()[:5]] = m.group(6)
    return lines[0], tally, ll

H, F, B = (read(p) for p in sys.argv[1:4])
for name, (first, tally, _) in (("head", H), ("4ff4549", F), ("base", B)):
    print(f"## {name}: {first}")
    print("\n".join(tally))
    print()
h, f, b = H[2], F[2], B[2]
print(f"livelocks: head {len(h)}, 4ff4549 {len(f)}, base {len(b)}")
print(f"at the head and not at 4ff4549: {len(set(h) - set(f))}; not at the base: {len(set(h) - set(b))}")
gone = set(f) - set(h)
print(f"at 4ff4549 and not at the head (ended by 26e83c5): {len(gone)}; with `stopped` as first or later answer: "
      f"{sum(1 for k in gone if 'Stopped' in k[1] + k[2])}")
print()

# What skyzfsd cannot answer (crates/skyzfs-upload/src/publish.rs and the
# control channel): `progress`/`withdraw` never answer `asked` or `busy`
# (door answers only); a `publish` asked anew carries a count read anew
# from `cuts`, which the same daemon does not refuse `rewound`, `state` or
# `attachment` for good, and the door never answers `taken`; `asked` at
# the door needs a request of the name that pends.
def why_not(k):
    _, first, later, fate, _ = k
    out = []
    if fate in ("Refused(Asked)", "Refused(Busy)"):
        out.append(f"progress answers {fate}")
    if later in ("Refused(Rewound)", "Refused(State)", "Refused(Attachment)", "Refused(Taken)", "Refused(Asked)"):
        out.append(f"every ask anew refused {later}")
    if first == "Refused(Taken)":
        out.append("the door answers taken")
    return "; ".join(out)

classes = defaultdict(list)
for k, trail in sorted(h.items()):
    classes[why_not(k) or "SKYZFSD COULD ANSWER SO"].append((k, trail))
print("## Livelocks left at the head, by what the script needs that skyzfsd does not answer")
for c, ks in sorted(classes.items(), key=lambda x: -len(x[1])):
    print(f"{len(ks):5} {c}")
    k, trail = ks[0]
    print(f"      e.g. {k}: {trail[:160]}")
print()
print("## Every livelock left at the head (publishes, age, first, later, fate, look)")
for k in sorted(h):
    print(" ".join(k))
