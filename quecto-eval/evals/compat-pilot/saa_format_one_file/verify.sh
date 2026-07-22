#!/bin/sh
grep -q '^  console' app.js && grep -q '^    console' server.js
