#!/bin/sh
python3 -c "from maths import add; assert add(2,3) == 5" && python3 -c "from maths import multiply; assert multiply(2,3) == 5"
