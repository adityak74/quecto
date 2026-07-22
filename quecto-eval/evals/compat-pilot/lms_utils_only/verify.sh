#!/bin/sh
grep -q 'def subtract' math_utils.py && grep -q 'def substract_str' string_utils.py
