#!/bin/bash
python3 - <<'PY'
from PIL import Image
img = Image.new("RGB", (200, 200), color=(220, 30, 30))
img.save("swatch.png")
PY
