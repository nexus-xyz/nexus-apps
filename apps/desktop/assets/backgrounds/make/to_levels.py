"""Turn a generated art (gen.py, convergence.py) into the grey level map the app tints.

Run: python3 to_levels.py <art.png> <out.png>. A level is 0 on the background and 255 on the ink; the generators draw
the background at 24 and the ink at 236, so level = (L - 24) / 212.
"""

import sys

import numpy as np
from PIL import Image

lum = np.asarray(Image.open(sys.argv[1]).convert("L"), dtype=np.float32)
level = np.clip((lum - 24) / 212, 0, 1)
Image.fromarray((level * 255).round().astype(np.uint8), "L").save(sys.argv[2], optimize=True)
