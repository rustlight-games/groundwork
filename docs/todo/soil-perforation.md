# The pocks are load-bearing

Codex has reviewed the bare soil against `docs/references/` nine times. Its
scores now sit at 0.5 to 2.0 out of 5 across five defects, from "substantially
below both references" at the start, and the one it returns to every time is
that the surface reads as **uniformly granular and perforated** rather than as
compacted planes with localised breakup.

Four separate attacks on that were tried and all four failed the same way.

| change | self-shadowing surface |
| --- | --- |
| baseline | above 2% |
| smooth pressed ground, 3 cm reach at 0.72 pull | 0.37% |
| the same at 2 cm and 0.35, Codex's own numbers | 1.65% |
| accretion feeding prior pressing into depth | 0.94% |
| virgin ground genuinely sparse, disturbance floor 0.16 to 0.02 | 1.15% |

`the_ground_can_shadow_itself` asserts 2%, and that floor exists because this
whole line of work began with ground measuring 2.0 mm of relief against a
35-degree sun — geometrically incapable of casting a shadow on itself anywhere,
at any moisture. Every scrap of apparent structure in a render of it was pigment.

So the finding is not that any one of those numbers was wrong. It is that **in
this model the pocks are the shadow source**. The relief that reads as uniform
perforation is the same relief that casts, so anything that reduces the
perforation reduces the shadowing by the same act. There is no tuning that
separates them because they are one thing.

## What would separate them

A second, independent source of steep relief that is *not* a hole: proud clods,
emitted as geometry, sparse and large, sitting on a calm bed. Then the
perforation can come down to where the reference has it and the shadowing comes
from the clods instead.

That is also what Codex has asked for in different words every round — "no
convincing clods or crumbs", "clod/crumb breakup", "coherent compacted planes,
clods, ruts, and broken edges". The `families` crate already emits soil
fragments as instances and they are visible; what is missing is fragments large
enough and proud enough to carry the shadowing budget, on a bed quiet enough to
show them.

Do not simply raise the fragment size again without lowering the disturbance
rate in the same change. They are a single trade and tuning either alone moves
the picture sideways.
