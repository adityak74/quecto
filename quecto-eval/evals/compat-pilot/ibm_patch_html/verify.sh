#!/bin/sh
grep -q 'content="test"' index.html && grep -q 'charset="utf-8"' index.html
