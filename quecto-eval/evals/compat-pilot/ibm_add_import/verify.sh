#!/bin/sh
grep -q 'import sys' script.py && grep -q 'import os' script.py
