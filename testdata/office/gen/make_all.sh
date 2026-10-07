#!/bin/bash
# Regenerates every file in testdata/office/ (LibreOffice 26.2 + openpyxl). See README.md.
set -e
cd "$(dirname "$0")"
for j in gen_pivot.py gen_objects.py gen_layout.py gen_formulas.py gen_formats.py gen_sheets.py gen_misc.py gen_word.py; do
  ./run_job.sh "$j" | grep -E "JOB-|FAILED|Traceback|Error" || true
done
python3 gen_openpyxl.py
