#!/bin/sh
python3 -c "import json; json.load(open('backend/config.json'))" && grep -q '{ "port": 3000 ' frontend/config.json
